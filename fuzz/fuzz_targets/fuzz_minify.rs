#![no_main]
//! On any input that parses, minify(minify(x)) == minify(x) and the
//! minified output re-parses clean.

use libfuzzer_sys::fuzz_target;
use luck_fuzz::{decode_input, with_arena};
use luck_minifier::TransformConfig;

fuzz_target!(|data: &[u8]| {
    let Some((source, version)) = decode_input(data) else {
        return;
    };
    with_arena(|allocator| {
        let config = TransformConfig::default();
        // `minify` fails only on a parse error, so this skips what doesn't
        // parse.
        let Ok(first) = luck_minifier::minify(allocator, source, version, &config) else {
            return;
        };
        let Ok(second) = luck_minifier::minify(allocator, &first, version, &config) else {
            panic!("minified output failed to reparse");
        };
        assert_eq!(first, second, "minify not idempotent");
    });
});
