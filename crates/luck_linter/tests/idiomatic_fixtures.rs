//! The `tests/fixtures/idiomatic/` corpus is written so that a default
//! lint run producing any diagnostic on it is a false-positive bug.

use luck_linter::{LintConfig, lint};
use luck_token::LuaVersion;
use std::path::PathBuf;

fn idiomatic_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has parent")
        .parent()
        .expect("crates dir has parent")
        .join("tests")
        .join("fixtures")
        .join("idiomatic")
}

#[test]
fn default_lint_is_silent_on_idiomatic_fixtures() {
    let entries = std::fs::read_dir(idiomatic_dir()).expect("idiomatic fixtures dir exists");
    let mut checked = 0;
    for entry in entries {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("lua") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("readable fixture");
        let diags = lint(&source, LuaVersion::Lua54, &LintConfig::default());
        assert!(
            diags.is_empty(),
            "false positives in {}: {diags:?}",
            path.display()
        );
        checked += 1;
    }
    assert!(checked >= 3, "expected idiomatic fixtures, found {checked}");
}
