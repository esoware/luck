use core::fmt::Write as _;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use luck_allocator::Allocator;
use luck_parser::{parse, parse_with_tokens};
use luck_testgen::{collect_fixture_files, detect_version, is_error_fixture};

fn fixtures_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has parent")
        .parent()
        .expect("crates dir has parent")
        .join("tests")
        .join("fixtures")
}

#[test]
fn fixtures_outside_errors_parse_cleanly() {
    let allocator = Allocator::new();
    let root = fixtures_directory();
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
        let source = fs::read_to_string(path).expect("failed to read fixture file");
        let result = parse(&allocator, source.as_bytes(), detect_version(relative));

        if is_error_fixture(relative) {
            // Fixtures under `errors/` may or may not parse.
            continue;
        }

        if result.errors.is_empty() {
            success_count = success_count.saturating_add(1);
        } else {
            failures.push((path.clone(), result.errors));
        }
    }

    if !failures.is_empty() {
        let mut report = format!("{} fixture(s) failed to parse:\n", failures.len());
        for (path, errors) in &failures {
            writeln!(report, "  {}:", path.display()).unwrap();
            for e in errors {
                writeln!(report, "    - {}", e.message).unwrap();
            }
        }
        panic!("{report}{success_count} fixture(s) parsed successfully");
    }

    assert!(success_count > 0, "parsed {success_count} fixtures successfully");
}

#[test]
fn every_fixture_parses_with_the_tokens_it_lexes_to() {
    let root = fixtures_directory();
    let files = collect_fixture_files(&root).expect("fixture tree is readable");
    for path in files
        .iter()
        .filter(|path| matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau")))
    {
        let relative = path.strip_prefix(&root).expect("fixture is under the root");
        let source = fs::read_to_string(path).expect("failed to read fixture file");
        let version = detect_version(relative);
        let allocator = Allocator::new();
        let (_, tokens) = parse_with_tokens(&allocator, source.as_bytes(), version);
        assert_eq!(
            tokens,
            luck_lexer::lex(source.as_bytes(), version).tokens,
            "{}",
            relative.display()
        );
    }
}
