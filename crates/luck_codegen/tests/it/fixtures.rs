use core::fmt::Write as _;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use luck_parser::parse;
use luck_testgen::{collect_fixture_files, detect_version, is_error_fixture};

#[test]
fn compact_output_of_every_fixture_reparses() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let files: Vec<PathBuf> = collect_fixture_files(&root)
        .expect("fixture tree is readable")
        .into_iter()
        .filter(|path| matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau")))
        .collect();
    assert!(!files.is_empty(), "no fixture files found");

    let mut failures = Vec::new();
    let mut success_count = 0usize;

    for path in &files {
        let relative = path.strip_prefix(&root).expect("fixture is under the root");
        if is_error_fixture(relative) {
            continue;
        }
        let source = fs::read_to_string(path).expect("failed to read fixture file");
        let version = detect_version(relative);
        let parsed = parse(&source, version);
        if !parsed.errors.is_empty() {
            continue;
        }

        let compacted = luck_codegen::compact(&parsed.block);
        let reparsed = parse(&compacted, version);
        if reparsed.errors.is_empty() {
            success_count += 1;
        } else {
            failures.push((path.clone(), compacted, reparsed.errors));
        }
    }

    if !failures.is_empty() {
        let mut report = format!("{} fixture(s) failed compact roundtrip:\n", failures.len());
        for (path, compacted, errors) in &failures {
            writeln!(report, "  {}:", path.display()).unwrap();
            writeln!(report, "    compact output: {compacted}").unwrap();
            for error in errors {
                writeln!(report, "    - {}", error.message).unwrap();
            }
        }
        panic!("{report}{success_count} fixture(s) passed compact roundtrip");
    }

    assert!(success_count > 0, "compact roundtrip passed for {success_count} fixtures");
}
