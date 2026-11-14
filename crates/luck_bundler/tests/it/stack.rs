//! Bundling fits the smallest stack.
//!
//! The bundler parses, analyzes, rewrites, and prints every module, and the
//! minifier takes the tree it builds, so a module nested to the parser's cap,
//! or holding long runs and lists, gets through all of that.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use std::fs;

use luck_bundler::{BundleOptions, DynamicRequire, bundle_block};
use luck_minifier::TransformConfig;
use luck_testgen::{deep_and_long_sources, on_the_smallest_stack};
use tempfile::TempDir;

use crate::common::bundle_both;

#[test]
fn bundling_fits_the_smallest_stack() {
    on_the_smallest_stack(deep_and_long_sources(), |source, version| {
        let directory = TempDir::new().expect("directory");
        let (entry_name, module_name, require) = if version.is_luau() {
            ("main.luau", "m.luau", "require(\"./m\")")
        } else {
            ("main.lua", "m.lua", "require(\"m\")")
        };
        fs::write(directory.path().join(module_name), source).expect("write the module");
        let entry = directory.path().join(entry_name);
        let entry_source = format!(
            "local m = {require}
print(m)
"
        );
        fs::write(&entry, entry_source).expect("write the entry");
        let search_paths = if version.is_luau() { vec![] } else { vec!["?.lua".to_owned()] };
        let options = BundleOptions {
            version,
            search_paths: &search_paths,
            project_root: directory.path(),
            dynamic_require: DynamicRequire::default(),
        };
        match bundle_both(&entry, &options) {
            Ok(_) => {
                let tree = bundle_block(&entry, &options).expect("the tree bundles too");
                luck_minifier::minify_block(tree.block, version, &TransformConfig::default())
                    .expect("the tree minifies");
            }
            // A module nested to the cap passes it inside its wrapper.
            Err(errors) => {
                assert!(errors.iter().all(|error| error.code == "E013"), "{errors:?}");
            }
        }
    });
}
