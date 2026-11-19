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
pub use types::TypeFieldSpec;

use core::sync::atomic::{AtomicU64, Ordering};

use luck_allocator::{Allocator, Box, List, ThinStr};
use luck_token::{LuaVersion, Span};

use crate::leaf::{Literal, Name};

/// Node constructors for trees built without source text.
///
/// They're built for tools that emit a tree directly, such as decompilers,
/// so they also take on the output-validity work such a tool would otherwise
/// reimplement. They build every node with the tree's generated
/// constructors, such as [`Expression::new_binary_op`](crate::Expression::new_binary_op),
/// which take each field as given and check nothing. Nodes go in the
/// [`Allocator`] the synthesizer is given, and every name and literal is
/// copied into it. Lists come in as arrays or as iterators of known length.
///
/// `Synth` is `Copy`, and its methods take `&self`, so calls nest:
/// `synth.call(synth.name_expr("f"), [synth.nil()])`.
///
/// # Spans
///
/// Every synthesized node gets a fresh single-point span `Span::new(n, n)`
/// from the [`SpanCounter`] the synthesizer is given, with `n` strictly
/// increasing until it reaches `u32::MAX`, after which every later node gets
/// that offset instead of wrapping back to zero. These spans don't index into
/// any source; they only keep nodes distinguishable for comment anchoring and
/// diagnostics. Spans are unique among every synthesizer that shares a
/// counter, so when several feed one tree, give them one. An arena isn't
/// shared across threads, but a counter is: a synthesizer on each thread,
/// each with its own arena and all with one counter, can build the pieces of
/// one tree in parallel, and the pieces join into it unchanged, since a tree
/// may hold nodes from any number of arenas that outlive it. To splice
/// synthetic nodes into a parsed tree, start the counter above the source's
/// byte offsets with [`SpanCounter::starting_at`], or comment anchors will
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
///   quoted literal text. Decimal escapes are always three digits, so a
///   following literal digit can't extend them. Interpolated-string text
///   escapes `` ` ``, `{`, and `\`. [`Synth::long_string`] falls back to the
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
/// - **Loud failure on invalid names.** [`Synth::name`] asserts, in release
///   builds too, that its argument is an identifier, so hostile input such
///   as bytecode debug info or obfuscated names fails fast instead of
///   producing output that won't re-parse. Every constructor that takes a
///   name goes through it and says so under `# Panics`.
/// - **Runs.** A run of suffixes, or of binary operators at one level, is one
///   level of nesting however long it gets, and the constructors join a run
///   they extend rather than nest it. Extending a run at its end takes
///   amortized constant time and arena space, and extending a
///   right-associative one (`..` or `^`) at its start shifts every link it
///   holds.
/// - **Block ends.** [`Synth::block`] puts a `return`, a `continue`, or a
///   `break` that the dialect takes only at the end of a block, but that has
///   statements after it, in a `do ... end` of its own.
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
///   number rendering, and block ends to one dialect, but deliberately
///   doesn't gate constructs.
/// - **Identifier validity.** For names that may not be identifiers (such as
///   bytecode debug info), build fields with [`Synth::field_or_index`] and
///   table keys with [`Synth::record`], which fall back to the bracketed
///   string form. Other name positions (locals, params, labels, function and
///   method names) have no bracketed spelling, so rewrite those names with
///   [`sanitize_identifier`] / [`sanitize_identifier_in`], or check them with
///   [`is_valid_identifier`] / [`is_valid_identifier_in`] and rename them
///   yourself. Sanitized names are deterministic but can collide, so
///   deduplication is up to the caller.
/// - **Raw text.** [`Synth::number`] and [`Synth::integer_literal`] take
///   numeral text as written. The printers print each leaf as the tree holds
///   it, so text that isn't one numeral prints output that doesn't read back
///   as the tree. The same goes for a node built with its own constructor,
///   with spans from [`Synth::next_span`].
/// - **Type names.** A type reads a bare `typeof` as the start of
///   `typeof(expr)`, so [`Synth::ty_named`] or [`Synth::ty_generic`] given
///   `typeof` prints a type that doesn't read back. Don't build one: a type
///   named `typeof` can be referred to only through its module, with
///   [`Synth::ty_qualified`], or as a pack, with [`Synth::ty_generic_pack`].
/// - **Nesting depth.** No constructor counts it. Every traversal recurses
///   once per level of nesting, so keep the tree within
///   [`MAX_NESTING_DEPTH`](crate::MAX_NESTING_DEPTH), the deepest
///   `luck_parser` builds. [`block_nesting_depth`](crate::block_nesting_depth)
///   and [`expr_nesting_depth`](crate::expr_nesting_depth) measure a tree of
///   any depth without recursing past the cap.
///
/// To print the result, put its block in a [`Chunk`](crate::Chunk) with the
/// version it's written in, and pass that to `luck_formatter::format_chunk`,
/// with any [`SyntheticComment`]s wrapped in `Comments::synthetic`, or to
/// `luck_codegen::compact` for compact output. Both print each leaf as the
/// tree holds it. `format_chunk` returns an error for a tree nested past the
/// cap, and `compact` prints it as it is, output the parser refuses.
///
/// # Usage
///
/// ```
/// use luck_allocator::Allocator;
/// use luck_ast::{SpanCounter, Synth};
/// use luck_token::BinOp;
///
/// let allocator = Allocator::new();
/// let spans = SpanCounter::new();
/// let synth = Synth::new(&allocator, &spans);
/// // total = a + b * 2 -- precedence handled, no manual parens.
/// let sum = synth.binary_op(
///     synth.name_expr("a"),
///     BinOp::Add,
///     synth.binary_op(synth.name_expr("b"), BinOp::Mul, synth.number_i64(2)),
/// );
/// let stmt = synth.assign([synth.name_var("total")], [sum]);
/// let block = synth.block([stmt]);
/// assert_eq!(block.stmts.len(), 1);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Synth<'ast> {
    allocator: &'ast Allocator,
    spans: &'ast SpanCounter,
    version: Option<LuaVersion>,
}

/// Where a [`Synth`] takes each node's span from, shared by every synthesizer
/// whose nodes join one tree, on any number of threads.
#[derive(Debug, Default)]
pub struct SpanCounter {
    /// Wider than an offset, so it can't wrap before every offset is given
    /// out.
    next_offset: AtomicU64,
}

impl SpanCounter {
    #[must_use]
    pub const fn new() -> Self {
        Self { next_offset: AtomicU64::new(0) }
    }

    /// A counter whose spans start at `offset`, for splicing nodes into a
    /// parsed tree without colliding with its byte offsets.
    #[must_use]
    pub fn starting_at(offset: u32) -> Self {
        Self { next_offset: AtomicU64::new(u64::from(offset)) }
    }

    /// Relaxed is enough: uniqueness needs atomicity, not ordering. Offsets
    /// stop at `u32::MAX` instead of wrapping, since a wrapped counter would
    /// hand out offsets that collide with earlier nodes or real source
    /// positions. Repeating `u32::MAX` keeps synthesis infallible and only
    /// costs comment placement.
    fn next_span(&self) -> Span {
        let counted = self.next_offset.fetch_add(1, Ordering::Relaxed);
        let offset = u32::try_from(counted).unwrap_or(u32::MAX);
        Span::new(offset, offset)
    }
}

impl<'ast> Synth<'ast> {
    #[must_use]
    pub const fn new(allocator: &'ast Allocator, spans: &'ast SpanCounter) -> Self {
        Self { allocator, spans, version: None }
    }

    /// Pins name handling and number rendering to one dialect.
    ///
    /// [`Synth::name`] checks names against that dialect's keywords instead
    /// of only the ones every dialect reserves. [`Synth::field_or_index`] and
    /// [`Synth::record`] use the dot or named form for any name that's an
    /// identifier there (`t.goto` on Luau) instead of falling back to
    /// brackets. [`Synth::number_f64`] and [`Synth::number_i64`] render for
    /// that dialect's number model, and [`Synth::block`] leaves a `break`
    /// that dialect takes anywhere as it is. It deliberately doesn't gate
    /// which constructs you can build.
    #[must_use]
    pub const fn with_version(mut self, version: LuaVersion) -> Self {
        self.version = Some(version);
        self
    }

    /// A fresh single-point span, distinct from every earlier one the counter
    /// gave out until it reaches `u32::MAX`, for a node built with its own
    /// constructor.
    #[must_use]
    pub fn next_span(&self) -> Span {
        self.spans.next_span()
    }

    /// Asserts validity in release builds too, so an untrusted name fails
    /// loudly instead of producing output that won't re-parse. Names that may
    /// not be identifiers belong in [`Synth::field_or_index`],
    /// [`Synth::record`], or [`Synth::string`], or need
    /// [`sanitize_identifier`] / [`sanitize_identifier_in`] first.
    ///
    /// # Panics
    ///
    /// Panics if `text` isn't an identifier under the pinned version, or in
    /// any dialect when none is pinned.
    #[must_use]
    pub fn name(&self, text: &str) -> Name<'ast> {
        let is_valid = self.version.map_or_else(
            || is_valid_identifier(text),
            |version| is_valid_identifier_in(text, version),
        );
        assert!(is_valid, "synth: {text:?} is not a valid identifier");
        Name::new(ThinStr::new(self.allocator.alloc_str(text)), None, self.next_span())
    }

    /// Whether `name` works as a `.name` field or `name =` key: an identifier
    /// in the pinned dialect, or in every dialect when none is pinned.
    fn is_safe_field_name(&self, name: &str) -> bool {
        self.version.map_or_else(
            || LuaVersion::ALL.iter().all(|&version| is_valid_identifier_in(name, version)),
            |version| is_valid_identifier_in(name, version),
        )
    }

    fn list<T, Items>(&self, items: Items) -> List<'ast, T>
    where
        Items: IntoIterator<Item = T>,
        Items::IntoIter: ExactSizeIterator,
    {
        List::from_iter_in(items, self.allocator)
    }

    fn boxed<T>(&self, value: T) -> Box<'ast, T> {
        Box::new_in(value, self.allocator)
    }

    /// `text` copied into the arena, at a fresh span.
    fn literal(&self, text: &str) -> Literal<'ast> {
        Literal::new(self.allocator.alloc_str(text), self.next_span())
    }
}

#[cfg(test)]
mod tests {
    use core::iter::repeat_with;
    use std::thread;

    use rustc_hash::FxHashSet;

    use super::*;
    use crate::expr::{Expression, FunctionArgs, Suffix};
    use crate::fixtures::Fixture;
    use crate::span::GetSpan as _;
    use crate::stmt::Statement;

    #[test]
    fn spans_are_monotonic() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let first = synth.nil();
        let second = synth.nil();
        let third = synth.number("3");
        assert!(second.span().start > first.span().start);
        assert!(third.span().start > second.span().start);
    }

    #[test]
    fn starting_at_offsets_spans() {
        let allocator = Allocator::new();
        let spans = SpanCounter::starting_at(1000);
        assert_eq!(Synth::new(&allocator, &spans).nil().span().start, 1000);
    }

    #[test]
    fn exhausted_spans_never_wrap_even_when_shared() {
        let (first, second) = (Allocator::new(), Allocator::new());
        let spans = SpanCounter::starting_at(u32::MAX - 1);
        let synth = Synth::new(&first, &spans);
        let shared = Synth::new(&second, &spans);
        assert_eq!(synth.nil().span().start, u32::MAX - 1);
        for synthesizer in [&synth, &shared, &synth] {
            assert_eq!(synthesizer.nil().span().start, u32::MAX);
        }
    }

    #[test]
    fn exhaustion_under_contention_never_restarts_the_range() {
        // A check-then-increment guard would race here: one thread can wrap
        // the counter to zero while another is mid-allocation, handing out
        // offsets that collide with the range's first nodes.
        let start = u32::MAX - 4;
        let spans = SpanCounter::starting_at(start);
        let offsets: Vec<u32> = thread::scope(|scope| {
            let workers: Vec<_> = repeat_with(|| {
                scope.spawn(|| {
                    let allocator = Allocator::new();
                    let synth = Synth::new(&allocator, &spans);
                    repeat_with(|| synth.nil().span().start).take(4).collect::<Vec<u32>>()
                })
            })
            .take(4)
            .collect();
            workers.into_iter().flat_map(|worker| worker.join().unwrap()).collect()
        });
        assert!(offsets.iter().all(|&offset| offset >= start), "{offsets:?}");
        // Only `u32::MAX` may repeat; every other offset is distinct however
        // the threads interleave.
        let allocated: Vec<u32> =
            offsets.iter().copied().filter(|&offset| offset < u32::MAX).collect();
        let unique: FxHashSet<u32> = allocated.iter().copied().collect();
        assert_eq!(unique.len(), allocated.len(), "{offsets:?}");
        assert_eq!(allocated.len(), 4, "{offsets:?}");
    }

    #[test]
    fn synths_sharing_a_counter_build_one_tree_across_threads() {
        let spans = SpanCounter::new();
        let mut arenas: Vec<Allocator> = repeat_with(Allocator::new).take(4).collect();
        let main_arena = Allocator::new();
        let synth = Synth::new(&main_arena, &spans);
        let shared = &spans;
        let pieces: Vec<Expression<'_>> = thread::scope(|scope| {
            let workers: Vec<_> = arenas
                .iter_mut()
                .map(|arena| {
                    scope.spawn(move || {
                        let worker = Synth::new(arena, shared);
                        let args = repeat_with(|| worker.nil()).take(250).collect::<Vec<_>>();
                        worker.call(worker.name_expr("f"), args)
                    })
                })
                .collect();
            workers.into_iter().map(|worker| worker.join().unwrap()).collect()
        });
        let tree = synth.block([synth.return_(pieces)]);
        let mut offsets = Vec::new();
        let [Statement::Return(returned)] = &*tree.stmts else {
            panic!("expected a return");
        };
        for piece in &returned.exprs {
            let Expression::Suffixed(chain) = piece else {
                panic!("expected a call, found {piece:?}");
            };
            offsets.push(chain.span.start);
            let Suffix::Call(FunctionArgs::Parenthesized { args, .. }) = chain.suffixes.first()
            else {
                panic!("expected parenthesized arguments");
            };
            offsets.extend(args.iter().map(|arg| arg.span().start));
        }
        let unique: FxHashSet<u32> = offsets.iter().copied().collect();
        assert_eq!(unique.len(), offsets.len());
        assert_eq!(offsets.len(), 4 * 251);
    }

    #[test]
    fn with_version_pins_field_names() {
        let fixture = Fixture::default();
        let synth = fixture.synth().with_version(LuaVersion::Luau);
        // `goto` is an identifier only under the pinned Luau dialect, so the
        // dot form proves the pin.
        let Expression::Suffixed(chain) = synth.field_or_index(synth.name_expr("t"), "goto") else {
            panic!("expected a field");
        };
        assert!(matches!(chain.suffixes.last(), Suffix::Field(_)));
    }

    #[test]
    fn names_are_copied_into_the_arena() {
        let fixture = Fixture::default();
        let text = String::from("local_name");
        let name = fixture.synth().name(&text);
        drop(text);
        assert_eq!(name.text, "local_name");
    }

    #[test]
    fn span_counter_is_send_and_sync() {
        fn assert_send_sync<T>()
        where
            T: Send + Sync,
        {
        }
        assert_send_sync::<SpanCounter>();
    }
}
