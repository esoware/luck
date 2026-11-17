#![expect(
    unsafe_code,
    reason = "a parse borrows its arena and an analysis its parse, which one value can't say"
)]

use core::fmt;
use core::mem::ManuallyDrop;
use core::ptr::{self, NonNull};

use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_linter::{Analysis, LintConfig};
use luck_parser::ParseResult;
use luck_token::Token;

/// A text's arena, its parse there, and the analysis of that parse, which
/// nothing changes once they're built.
///
/// The parse and the analysis are lent out only behind `&self`, and the arena
/// never is, so nothing allocates in it again and every borrow ends before it
/// drops. The fields drop in order, each before what it borrows.
pub(crate) struct Analyzed {
    analysis: Analysis<'static>,
    parse: ArenaParse,
    #[expect(dead_code, reason = "held only so the arena drops after what lives in it")]
    allocator: Allocator,
}

/// A parse kept in its own arena, where it stays put however the
/// [`Analyzed`] holding it moves, as the analysis needs it to.
struct ArenaParse(NonNull<ParseResult<'static>>);

// SAFETY: It owns the parse it points to, as a `Box` would, and a parse is
// `Send`.
unsafe impl Send for ArenaParse {}

// SAFETY: `&Self` reaches only `&ParseResult`, and a parse is `Sync`.
unsafe impl Sync for ArenaParse {}

impl Drop for ArenaParse {
    fn drop(&mut self) {
        // SAFETY: The analysis that borrowed the parse dropped before this,
        // and the arena the parse lives in drops after it.
        unsafe {
            ptr::drop_in_place(self.0.as_ptr());
        }
    }
}

/// The parts other threads can reach, which makes the impls below hold.
const _: () = {
    const fn assert_shareable<T>()
    where
        T: Send + Sync,
    {
    }
    assert_shareable::<ParseResult<'static>>();
    assert_shareable::<Analysis<'static>>();
};

// SAFETY: The arena is the one part that isn't `Sync`, and `&Self` never
// reaches it, only the parse and the analysis, so no two threads can
// allocate in it. They hold the arena's chunks as a `Box` holds its value,
// so moving them with it to another thread moves nothing they borrow.
unsafe impl Sync for Analyzed {}

impl Analyzed {
    /// Copies `text` into a new arena, then parses and analyzes it there for
    /// `target`, with `config`'s extra globals defined. The tokens the parse
    /// read come back beside it.
    pub(crate) fn new(text: &str, target: LuaTarget, config: &LintConfig) -> (Self, Vec<Token>) {
        let allocator = Allocator::new();
        // SAFETY: What's built here lives in the arena's chunks, which stay
        // where they are when `allocator` moves into `Self`, and which drop
        // only after the parse and the analysis do. Nothing built keeps this
        // reference to `allocator` itself.
        let arena: &'static Allocator = unsafe { NonNull::from(&allocator).as_ref() };
        let source = arena.alloc_str(text);
        let (parsed, tokens) = luck_parser::parse_with_tokens(arena, source, target.lua_version());
        let parse = NonNull::from(arena.alloc(ManuallyDrop::new(parsed))).cast::<ParseResult<'_>>();
        // SAFETY: `parse` was just written, and only `ArenaParse` drops it.
        let analysis = luck_linter::analyze(unsafe { parse.as_ref() }, target, config);
        (Self { analysis, parse: ArenaParse(parse), allocator }, tokens)
    }

    pub(crate) const fn parse(&self) -> &ParseResult<'_> {
        // SAFETY: The parse lives until `self` drops, and the borrow shortens
        // the arena's lifetime to `self`'s.
        unsafe { self.parse.0.as_ref() }
    }

    pub(crate) const fn analysis(&self) -> &Analysis<'_> {
        &self.analysis
    }
}

impl fmt::Debug for Analyzed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Analyzed")
            .field("parse", self.parse())
            .field("analysis", self.analysis())
            .finish_non_exhaustive()
    }
}
