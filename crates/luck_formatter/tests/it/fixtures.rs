//! This covers what the hand-written cases pin one at a time (AST
//! equivalence, comment text and placement, and idempotency) over real files
//! in every dialect.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use luck_formatter::FormatOptions;
use luck_testgen::{collect_fixture_files, detect_version, is_error_fixture};

#[test]
fn every_fixture_formats_and_verifies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let fixtures = collect_fixture_files(&root).expect("fixture tree is readable");
    assert!(!fixtures.is_empty(), "no fixtures under {}", root.display());

    for path in &fixtures {
        let relative = path.strip_prefix(&root).expect("fixture is under the root");
        if is_error_fixture(relative)
            || !matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau"))
        {
            continue;
        }
        let version = detect_version(relative);
        let source = fs::read_to_string(path).expect("fixture is readable");
        for line_width in [1, 40, 80, 120] {
            let options = FormatOptions { line_width, ..FormatOptions::default() };
            let result = luck_formatter::format_and_verify(&source, version, &options)
                .unwrap_or_else(|(_, diff)| {
                    panic!("{} at width {line_width}: {diff:?}", path.display())
                });
            assert!(
                result.errors.is_empty(),
                "{} does not parse as {version:?}: {:?}",
                path.display(),
                result.errors
            );
        }
    }
}
