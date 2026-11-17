use std::cell::RefCell;

use luck_allocator::Allocator;
use luck_token::LuaVersion;

/// The source text and target version a fuzz input encodes, or `None` when
/// it isn't UTF-8. The input's length picks the version.
#[must_use]
pub fn decode_input(data: &[u8]) -> Option<(&str, LuaVersion)> {
    let source = std::str::from_utf8(data).ok()?;
    Some((source, LuaVersion::ALL[data.len() % LuaVersion::ALL.len()]))
}

/// Runs `run` in this thread's arena, reset first, so every run reuses the
/// memory the runs before it grew.
pub fn with_arena<R>(run: impl FnOnce(&Allocator) -> R) -> R {
    thread_local! {
        static ARENA: RefCell<Allocator> = RefCell::new(Allocator::new());
    }
    ARENA.with_borrow_mut(|allocator| {
        allocator.reset();
        run(allocator)
    })
}
