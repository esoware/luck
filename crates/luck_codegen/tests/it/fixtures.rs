use core::fmt::Write as _;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use luck_allocator::Allocator;
use luck_testgen::{collect_fixture_files, detect_version, is_error_fixture};

#[test]
fn every_fixture_compacts_to_output_that_compacts_to_itself() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let mut report = String::new();
    let mut round_trip_count = 0usize;
    for path in collect_fixture_files(&root).expect("the fixture tree is readable") {
        let relative = path.strip_prefix(&root).expect("a fixture is under the root");
        if is_error_fixture(relative)
            || !matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau"))
        {
            continue;
        }
        let source = fs::read_to_string(&path).expect("a fixture is readable");
        let version = detect_version(relative);
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, &source, version);
        if !parsed.errors.is_empty() {
            continue;
        }
        let output = luck_codegen::compact(&parsed.block);
        let reparsed = luck_parser::parse(&allocator, &output, version);
        if let Some(error) = reparsed.errors.first() {
            writeln!(report, "{}: {output}\n  {}", relative.display(), error.message).unwrap();
        } else if luck_codegen::compact(&reparsed.block) == output {
            round_trip_count += 1;
        } else {
            writeln!(report, "{}: compacting again changed {output}", relative.display()).unwrap();
        }
    }
    assert!(report.is_empty(), "{report}{round_trip_count} fixture(s) round-tripped");
    assert!(round_trip_count > 0, "no fixture parsed");
}
