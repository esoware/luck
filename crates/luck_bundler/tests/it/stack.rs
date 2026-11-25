//! Bundling fits the smallest stack.
//!
//! The bundler parses, analyzes, rewrites, prints, and minifies every module,
//! so a module nested to the parser's cap, or holding long runs and lists,
//! gets through all of that.

use luck_testgen::{deep_and_long_sources, on_the_smallest_stack};

use crate::common::{bundle_files, codes};

#[test]
fn bundling_fits_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source| {
        let entry_source = "\
local m = require(\"./m\")
print(m)
";
        let files = [("m.luau", source.as_bytes()), ("main.luau", entry_source.as_bytes())];
        // A module nested to the cap passes it inside its wrapper.
        if let Err(errors) = bundle_files(&files, "main.luau") {
            assert!(codes(&errors).iter().all(|&code| code == "E013"), "{errors:?}");
        }
    });
}
