//! Bundling fits the smallest stack.
//!
//! The bundler parses, analyzes, rewrites, prints, and minifies every module,
//! so a module nested to the parser's cap, or holding long runs and lists,
//! gets through all of that.

use luck_testgen::{deep_and_long_sources, on_the_smallest_stack};

use crate::common::{bundle_files, codes};

#[test]
fn bundling_fits_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source, version| {
        let (entry_name, module_name, require) = if version.is_luau() {
            ("main.luau", "m.luau", "require(\"./m\")")
        } else {
            ("main.lua", "m.lua", "require(\"m\")")
        };
        let entry_source = format!(
            "\
local m = {require}
print(m)
"
        );
        let files = [(module_name, source.as_bytes()), (entry_name, entry_source.as_bytes())];
        // A module nested to the cap passes it inside its wrapper.
        if let Err(errors) = bundle_files(&files, entry_name, version) {
            assert!(codes(&errors).iter().all(|&code| code == "E013"), "{errors:?}");
        }
    });
}
