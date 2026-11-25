//! Any diagnostic a default lint run reports on `tests/fixtures/idiomatic/`
//! is a false positive.

use luck_linter::LintConfig;
use luck_testgen::source_fixtures;

use crate::common::lint;

#[test]
fn default_lint_is_silent_on_idiomatic_fixtures() {
    let mut fixture_count = 0;
    for fixture in source_fixtures() {
        if !fixture.relative_path.starts_with("idiomatic") {
            continue;
        }
        let diags = lint(&fixture.source, &LintConfig::default());
        assert!(diags.is_empty(), "false positives in {}: {diags:?}", fixture.path.display());
        fixture_count += 1;
    }
    assert!(fixture_count >= 3, "expected idiomatic fixtures, found {fixture_count}");
}
