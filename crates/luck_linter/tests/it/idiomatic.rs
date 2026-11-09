//! Any diagnostic a default lint run reports on `tests/fixtures/idiomatic/`
//! is a false positive.

use std::fs::{read_dir, read_to_string};

use luck_linter::{LintConfig, lint};
use luck_token::LuaVersion;

const IDIOMATIC_DIRECTORY: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/idiomatic");

#[test]
fn default_lint_is_silent_on_idiomatic_fixtures() {
    let entries = read_dir(IDIOMATIC_DIRECTORY).expect("idiomatic fixtures dir exists");
    let mut fixture_count = 0usize;
    for entry in entries {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("lua") {
            continue;
        }
        let source = read_to_string(&path).expect("readable fixture");
        let diags = lint(&source, LuaVersion::Lua54, &LintConfig::default());
        assert!(diags.is_empty(), "false positives in {}: {diags:?}", path.display());
        fixture_count += 1;
    }
    assert!(fixture_count >= 3, "expected idiomatic fixtures, found {fixture_count}");
}
