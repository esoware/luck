#![no_main]
//! On any input that parses, format(format(x)) == format(x) and the
//! formatted output re-parses clean.

use libfuzzer_sys::fuzz_target;
use luck_formatter::FormatOptions;
use luck_fuzz::{decode_input, with_arena};

fuzz_target!(|data: &[u8]| {
    let Some((source, version)) = decode_input(data) else {
        return;
    };
    with_arena(|allocator| {
        let options = FormatOptions::default();
        // Formatting reports only parse errors, so this skips what doesn't
        // parse.
        let first = luck_formatter::format(allocator, source, version, &options);
        if !first.errors.is_empty() {
            return;
        }
        let second = luck_formatter::format(allocator, &first.output, version, &options);
        assert!(second.errors.is_empty(), "formatted output failed to reparse");
        assert_eq!(first.output, second.output, "format not idempotent on its own output");
    });
});
