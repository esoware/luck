#![expect(
    clippy::mem_forget,
    reason = "`self_cell!`'s builders forget their drop guard once the cell is built"
)]

use alloc::sync::Arc;
use core::{fmt, mem};

use luck_allocator::Allocator;
use luck_linter::{Analysis, LintConfig};
use luck_parser::ParseResult;
use luck_token::LuaTarget;
use parking_lot::Mutex;
use self_cell::{MutBorrow, self_cell};

/// A text, its parse in an arena of its own, and the analysis of that parse,
/// which nothing changes once they're built.
///
/// A parse borrows its arena and an analysis its parse, so they're two cells,
/// as `oxc_semantic` holds its arena and scoping. The arena sits in a
/// [`MutBorrow`], which lends it once, to the parse, so nothing can reach it
/// again from another thread, and the whole is `Sync` with no `unsafe`.
pub(crate) struct Analyzed(AnalyzedCell);

struct Source {
    text: Arc<str>,
    arena: MutBorrow<Arena>,
}

self_cell!(
    struct ParsedCell {
        owner: Source,
        #[covariant]
        dependent: ParseResult,
    }
);

self_cell!(
    struct AnalyzedCell {
        owner: ParsedCell,
        #[covariant]
        dependent: Analysis,
    }
);

impl Analyzed {
    /// Parses `text` in `arena`, keeping its tokens, and analyzes it for
    /// `target`, with `config`'s extra globals defined.
    pub(crate) fn new(
        text: Arc<str>,
        target: LuaTarget,
        config: &LintConfig,
        arena: Arena,
    ) -> Self {
        let owner = Source { text, arena: MutBorrow::new(arena) };
        let parsed = ParsedCell::new(owner, |source| {
            let lent: &Arena = source.arena.borrow_mut();
            luck_parser::parse_with_tokens(
                &lent.allocator,
                source.text.as_bytes(),
                target.version(),
            )
        });
        let analyzed = AnalyzedCell::new(parsed, |cell| {
            luck_linter::analyze(cell.borrow_dependent(), target, config)
        });
        Self(analyzed)
    }

    /// The text an editor sent, which is UTF-8, and which the parse reads as
    /// bytes.
    pub(crate) fn text(&self) -> &str {
        &self.0.borrow_owner().borrow_owner().text
    }

    pub(crate) fn parse(&self) -> &ParseResult<'_> {
        self.0.borrow_owner().borrow_dependent()
    }

    pub(crate) fn analysis(&self) -> &Analysis<'_> {
        self.0.borrow_dependent()
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

/// The arena one version parses into, which goes back to the [`SpareArena`]
/// emptied once the version drops, on whatever thread drops it last.
#[derive(Debug)]
pub(crate) struct Arena {
    allocator: Allocator,
    spare: Arc<SpareArena>,
}

impl Arena {
    /// The spare's arena, or a new one when there's none to reuse.
    pub(crate) fn from_spare(spare: &Arc<SpareArena>) -> Self {
        let allocator = spare.slot.lock().take().unwrap_or_default();
        Self { allocator, spare: Arc::clone(spare) }
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        let mut allocator = mem::take(&mut self.allocator);
        allocator.reset();
        let mut slot = self.spare.slot.lock();
        if slot.is_none() {
            *slot = Some(allocator);
        }
    }
}

/// The emptied arena of the last version dropped, kept so the next version
/// parses into memory that's already mapped. A close frees it, so a closed
/// document's memory doesn't stay with the next.
#[derive(Debug, Default)]
pub(crate) struct SpareArena {
    slot: Mutex<Option<Allocator>>,
}

impl SpareArena {
    pub(crate) fn clear(&self) {
        drop(self.slot.lock().take());
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.slot.lock().is_none()
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    fn analyzed(text: &str, spare: &Arc<SpareArena>) -> Analyzed {
        let config = LintConfig::default();
        Analyzed::new(Arc::from(text), LuaTarget::Lua54, &config, Arena::from_spare(spare))
    }

    #[test]
    fn a_dropped_version_leaves_its_arena_to_the_next_and_a_clear_frees_it() {
        let spare = Arc::new(SpareArena::default());
        let first = analyzed("local one = 1\n", &spare);
        assert!(spare.is_empty(), "nothing was dropped yet");
        drop(first);
        assert!(!spare.is_empty(), "the first version left its arena");
        let second = analyzed("local two = 2\n", &spare);
        assert!(spare.is_empty(), "the second version took it");
        assert_eq!(second.text(), "local two = 2\n");
        assert_eq!(second.parse().source, b"local two = 2\n");
        drop(second);
        spare.clear();
        assert!(spare.is_empty(), "a clear frees the spare arena");
    }

    #[test]
    fn an_analysis_moved_to_another_thread_keeps_its_tree() {
        let spare = Arc::new(SpareArena::default());
        let shared = Arc::new(analyzed("local answer = 42", &spare));
        let reader = Arc::clone(&shared);
        let names = thread::spawn(move || {
            let tree = reader.analysis().semantic().scoping();
            tree.symbols()
                .iter()
                .map(|symbol| tree.name(symbol.name).to_owned())
                .collect::<Vec<_>>()
        })
        .join()
        .expect("the reader thread finishes");
        assert_eq!(names, ["answer"]);
        drop(shared);
        assert!(!spare.is_empty(), "the last holder's drop returned the arena");
    }
}
