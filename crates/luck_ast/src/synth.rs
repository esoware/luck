mod comment;
mod expr;
mod function;
mod identifier;
mod literal;
mod stmt;
mod types;

pub use comment::SyntheticComment;
pub use expr::FieldSpec;
pub use function::FunctionSignature;
pub use identifier::{
    is_valid_identifier, is_valid_identifier_in, sanitize_identifier, sanitize_identifier_in,
};
pub use literal::InterpolatedStringPart;
pub use types::{TypeFieldAccess, TypeFieldSpec};

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};

use luck_token::{CompactString, LuaVersion, Span, Token, TokenKind};

/// Node constructors backed by a monotonic span counter.
///
/// Builds `luck_ast` nodes without any source text. Writing the node structs
/// by hand means spelling out every `Token { kind, span }`; these
/// constructors do that for you. They're built for tools that emit an AST
/// directly, so they also take on the output-validity work such a tool would
/// otherwise reimplement.
///
/// Methods take `&self`, so calls nest:
/// `synth.call(synth.name_expr("f"), vec![synth.nil()])`.
///
/// # Spans
///
/// Every synthesized node gets a fresh single-point span `Span::new(n, n)`,
/// with `n` strictly increasing until the counter saturates at `u32::MAX`,
/// after which every later node gets that offset instead of wrapping back to
/// zero. These spans don't index into any source; they only keep nodes
/// distinguishable for comment anchoring and diagnostics. Spans are unique
/// within one `Synth` and across every synthesizer created from it with
/// [`Synth::share`], so when several synthesizers feed one tree (parallel
/// decompilation of function protos, say), have them share one counter. To
/// splice synthetic nodes into a parsed AST, use [`Synth::starting_at`] to
/// put them above the source's byte offsets, or comment anchors will
/// collide.
///
/// # What the constructors guarantee
///
/// - **Operator precedence.** [`Synth::binary_op`], [`Synth::unary_op`], and
///   [`Synth::type_cast`] parenthesize their operands whenever the printed
///   form would re-parse differently: precedence and associativity for
///   nested binary operators, `(-a)^b`, greedy Luau if-expressions,
///   `(a + b) :: T`, chained casts (`(a :: T) :: U`), and casts printed
///   before `<`, `&`, or `|`, which the type grammar would otherwise
///   swallow (`(a :: T) < b`).
/// - **Type composition.** [`Synth::ty_optional`], [`Synth::ty_union`], and
///   [`Synth::ty_intersection`] parenthesize union, intersection, and
///   function members. That keeps their meaning and respects Luau's ban on
///   mixing `|` and `&` (or `?` and `&`) without parentheses.
/// - **Prefix positions.** Call callees, method receivers, and index and
///   field prefixes are wrapped unless they're already a var, call,
///   parenthesized expression, or type instantiation: `("s"):rep(2)`,
///   `({}).x`, `(function() end)()`.
/// - **Escaping.** [`Synth::string`] and [`Synth::string_bytes`] produce the
///   quoted token text. Decimal escapes are always three digits, so a
///   following literal digit can't extend them. Interpolated-string segments
///   escape `` ` ``, `{`, and `\`. [`Synth::long_string`] falls back to the
///   quoted form for carriage returns and NUL bytes.
/// - **Numeric literals.** [`Synth::number_f64`] and [`Synth::number_i64`]
///   render any value, including those with no literal form: negatives
///   (unary minus node), infinities (`1/0`), NaN (`0/0`), and `i64::MIN`
///   (hex, which wraps to the intended integer on Lua 5.3+). Integral floats
///   get a `.0` suffix so they stay floats on 5.3+, and exponent form
///   replaces the plain one whenever it's shorter (`1e300`). On a version
///   pinned to a single-number dialect (5.1, 5.2, Luau), integral floats
///   drop the `.0`, `i64::MIN` becomes an exact negated decimal literal, and
///   other integers may round to the nearest double. NaN sign and payload
///   aren't preserved.
/// - **Loud failure on invalid names.** [`Synth::identifier`] asserts, in release
///   builds too, that its argument is an identifier, so hostile input such
///   as bytecode debug info or obfuscated names fails fast instead of
///   producing output that won't re-parse. Every constructor that takes a
///   name goes through it and says so under `# Panics`.
///
/// # What the caller still owes
///
/// - **Multi-value truncation.** `f(g())` spreads all of `g`'s results.
///   Wrap the expression in [`Synth::single_value`] when exactly one value is
///   meant at the end of a call, return, or assignment list. Only the caller
///   knows which positions need it.
/// - **Version gating.** Nothing stops you from synthesizing, say, a Lua 5.4
///   attribute for a Luau target, so pick constructors that match the
///   dialect. [`Synth::with_version`] pins name validity, field-name safety,
///   and number rendering to one dialect, but deliberately doesn't gate
///   constructs.
/// - **Identifier validity.** For names that may not be identifiers (such as
///   bytecode debug info), build fields with [`Synth::field_or_index`] and
///   table keys with [`Synth::record`], which fall back to the bracketed
///   string form. Other name positions (locals, params, labels, function and
///   method names) have no bracketed spelling, so rewrite those names with
///   [`sanitize_identifier`] / [`sanitize_identifier_in`], or check them with
///   [`is_valid_identifier`] / [`is_valid_identifier_in`] and rename them
///   yourself. Sanitized names are deterministic but can collide, so
///   deduplication is up to the caller.
/// - **Nesting depth.** No constructor counts it. Every traversal, drop and
///   clone included, recurses once per level of nesting, so keep the tree
///   within [`MAX_NESTING_DEPTH`](crate::MAX_NESTING_DEPTH), the deepest
///   `luck_parser` builds. A run of suffixes, or of binary operators at one
///   level, is one level however long it gets, and the constructors join a
///   run they extend rather than nest it.
///   [`block_nesting_depth`](crate::block_nesting_depth) and
///   [`expr_nesting_depth`](crate::expr_nesting_depth) measure a tree of any
///   depth without recursing past the cap. Both printers return an error for
///   a deeper tree, but dropping or cloning one can still overflow the stack.
///
/// To format the result, pass it to `luck_formatter::format_block`, with any
/// [`SyntheticComment`]s wrapped in `Comments::synthetic`. For compact
/// output, call `luck_codegen::compact`. Both return an error for a tree
/// nested past the cap.
///
/// # Usage
///
/// ```
/// use luck_ast::Synth;
/// use luck_token::BinOp;
///
/// let synth = Synth::new();
/// // total = a + b * 2 -- precedence handled, no manual parens.
/// let sum = synth.binary_op(
///     synth.name_expr("a"),
///     BinOp::Add,
///     synth.binary_op(synth.name_expr("b"), BinOp::Mul, synth.number_i64(2)),
/// );
/// let stmt = synth.assign(vec![synth.name_var("total")], vec![sum]);
/// let block = synth.block(vec![stmt], None);
/// assert_eq!(block.stmts.len(), 1);
/// ```
#[derive(Debug, Default)]
pub struct Synth {
    next_offset: Arc<AtomicU32>,
    version: Option<LuaVersion>,
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the synthesizer is one type with its constructors split by node family"
)]
impl Synth {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A synthesizer whose spans start at `offset`, for splicing nodes into a
    /// parsed AST without colliding with its byte offsets. For several
    /// synthesizers feeding one tree, use [`Synth::share`] instead of picking
    /// disjoint ranges.
    #[must_use]
    pub fn starting_at(offset: u32) -> Self {
        Self { next_offset: Arc::new(AtomicU32::new(offset)), version: None }
    }

    /// A synthesizer that shares this one's span counter and version pin.
    /// Spans never collide across the family, so give one to each thread when
    /// synthesizing in parallel.
    #[must_use]
    pub fn share(&self) -> Self {
        Self { next_offset: Arc::clone(&self.next_offset), version: self.version }
    }

    /// Pins name handling and number rendering to one dialect.
    ///
    /// [`Synth::identifier`] checks names against that dialect's keywords
    /// instead of only the ones every dialect reserves. [`Synth::field_or_index`]
    /// and [`Synth::record`] use the dot or named form for any name that's an
    /// identifier there (`t.goto` on Luau) instead of falling back to
    /// brackets. [`Synth::number_f64`] and [`Synth::number_i64`] render for
    /// that dialect's number model. It deliberately doesn't gate which
    /// constructs you can build.
    #[must_use]
    pub const fn with_version(mut self, version: LuaVersion) -> Self {
        self.version = Some(version);
        self
    }

    /// A fresh single-point span, distinct from every earlier one until the
    /// counter saturates.
    fn next_span(&self) -> Span {
        // Relaxed is enough: uniqueness needs atomicity, not ordering. The
        // loop stops at `u32::MAX` instead of wrapping, since a wrapped
        // counter would hand out offsets that collide with earlier nodes or
        // real source positions. Repeating `u32::MAX` keeps synthesis
        // infallible and only costs comment placement.
        let mut offset = self.next_offset.load(Ordering::Relaxed);
        while let Some(next) = offset.checked_add(1) {
            match self.next_offset.compare_exchange_weak(
                offset,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => offset = observed,
            }
        }
        Span::new(offset, offset)
    }

    #[must_use]
    pub fn token(&self, kind: TokenKind) -> Token {
        Token::new(kind, self.next_span())
    }

    /// Asserts validity in release builds too, so an untrusted name fails
    /// loudly instead of producing output that won't re-parse. Names that may
    /// not be identifiers belong in [`Synth::field_or_index`],
    /// [`Synth::record`], or [`Synth::string`], or need
    /// [`sanitize_identifier`] / [`sanitize_identifier_in`] first.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't an identifier under the pinned version, or in
    /// any dialect when none is pinned.
    #[must_use]
    pub fn identifier(&self, name: &str) -> Token {
        self.token(TokenKind::Identifier(self.identifier_name(name)))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't an identifier under the pinned version, or in
    /// any dialect when none is pinned.
    fn identifier_name(&self, name: &str) -> CompactString {
        let is_valid = self.version.map_or_else(
            || is_valid_identifier(name),
            |version| is_valid_identifier_in(name, version),
        );
        assert!(is_valid, "synth: {name:?} is not a valid identifier");
        CompactString::from(name)
    }

    /// Whether `name` works as a `.name` field or `name =` key: an identifier
    /// in the pinned dialect, or in every dialect when none is pinned.
    fn is_safe_field_name(&self, name: &str) -> bool {
        self.version.map_or_else(
            || LuaVersion::ALL.iter().all(|&version| is_valid_identifier_in(name, version)),
            |version| is_valid_identifier_in(name, version),
        )
    }
}

#[cfg(test)]
mod tests {
    use core::iter::repeat_with;
    use std::thread;

    use rustc_hash::FxHashSet;

    use super::*;
    use crate::expr::{Expression, Suffix};

    #[test]
    fn spans_are_monotonic() {
        let synth = Synth::new();
        let first = synth.nil();
        let second = synth.nil();
        let third = synth.number("3");
        assert!(second.span().start > first.span().start);
        assert!(third.span().start > second.span().start);
    }

    #[test]
    fn starting_at_offsets_spans() {
        let synth = Synth::starting_at(1000);
        assert_eq!(synth.nil().span().start, 1000);
    }

    #[test]
    fn exhausted_spans_never_wrap_even_when_shared() {
        let synth = Synth::starting_at(u32::MAX - 1);
        let shared = synth.share();
        assert_eq!(synth.nil().span().start, u32::MAX - 1);
        for allocator in [&synth, &shared, &synth] {
            assert_eq!(allocator.nil().span().start, u32::MAX);
            assert_eq!(allocator.next_offset.load(Ordering::Relaxed), u32::MAX);
        }
    }

    #[test]
    fn exhaustion_under_contention_never_restarts_the_range() {
        // A check-then-increment guard would race here: one thread can wrap
        // the counter to zero while another is mid-allocation, handing out
        // offsets that collide with the range's first nodes.
        let start = u32::MAX - 4;
        let synth = Synth::starting_at(start);
        let mut offsets: Vec<u32> = Vec::new();
        thread::scope(|scope| {
            let handles: Vec<_> = repeat_with(|| {
                let shared = synth.share();
                scope.spawn(move || {
                    repeat_with(|| shared.nil().span().start).take(4).collect::<Vec<u32>>()
                })
            })
            .take(4)
            .collect();
            for handle in handles {
                offsets.extend(handle.join().unwrap());
            }
        });
        assert!(offsets.iter().all(|&offset| offset >= start), "{offsets:?}");
        // Only `u32::MAX` may repeat; every other offset is distinct however
        // the threads interleave.
        let allocated: Vec<u32> =
            offsets.iter().copied().filter(|&offset| offset < u32::MAX).collect();
        let unique: FxHashSet<u32> = allocated.iter().copied().collect();
        assert_eq!(unique.len(), allocated.len(), "{offsets:?}");
        assert_eq!(synth.next_offset.load(Ordering::Relaxed), u32::MAX);
    }

    #[test]
    fn shared_synths_never_collide() {
        let synth = Synth::new();
        let shared = synth.share();
        let spans = [
            synth.nil().span().start,
            shared.nil().span().start,
            synth.nil().span().start,
            shared.nil().span().start,
        ];
        let unique: FxHashSet<u32> = spans.iter().copied().collect();
        assert_eq!(unique.len(), spans.len(), "{spans:?}");
    }

    #[test]
    fn shared_synths_never_collide_across_threads() {
        let synth = Synth::new();
        let mut all_spans: Vec<u32> = Vec::new();
        thread::scope(|scope| {
            let handles: Vec<_> = repeat_with(|| {
                let shared = synth.share();
                scope.spawn(move || {
                    repeat_with(|| shared.nil().span().start).take(250).collect::<Vec<u32>>()
                })
            })
            .take(4)
            .collect();
            for handle in handles {
                all_spans.extend(handle.join().unwrap());
            }
        });
        let unique: FxHashSet<u32> = all_spans.iter().copied().collect();
        assert_eq!(unique.len(), all_spans.len());
    }

    #[test]
    fn share_preserves_version_pin() {
        let shared = Synth::new().with_version(LuaVersion::Luau).share();
        // `goto` is an identifier only under the pinned Luau dialect, so the
        // dot form proves the pin carried over.
        let Expression::Suffixed(chain) = shared.field_or_index(shared.name_expr("t"), "goto")
        else {
            panic!("expected a field");
        };
        assert!(matches!(chain.suffixes.last(), Suffix::Field(_)));
    }

    #[test]
    fn synth_is_send_and_sync() {
        fn assert_send_sync<T>()
        where
            T: Send + Sync,
        {
        }
        assert_send_sync::<Synth>();
    }
}
