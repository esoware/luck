//! The parse's counts size every table analysis fills, so a clean parse
//! counts at least as many of each as analysis makes, and one scope per
//! block exactly.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use luck_allocator::Allocator;
use luck_testgen::{
    collect_fixture_files, detect_version, generate_full, generate_runtime_safe, is_error_fixture,
};
use luck_token::{LuaVersion, StdlibEnvironment};

fn assert_counts_cover_the_tables(source: &str, version: LuaVersion) {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, source, version);
    assert!(parsed.errors.is_empty(), "{:?}\n{source}", parsed.errors.first());
    let counts = parsed.counts;
    let block = allocator.alloc(parsed.block);
    let analysis = luck_semantic::analyze(block, counts, version, StdlibEnvironment::Standalone);
    let tree = analysis.scope_tree();
    let declarations = usize::try_from(counts.declarations).unwrap();
    let name_uses = usize::try_from(counts.name_uses).unwrap();
    assert_eq!(tree.scopes().len(), usize::try_from(counts.blocks).unwrap(), "{source}");
    assert!(tree.symbols().len() <= declarations, "{counts:?}\n{source}");
    assert!(tree.references().len() <= name_uses, "{counts:?}\n{source}");
    assert!(tree.names().count() <= declarations.saturating_add(name_uses), "{counts:?}\n{source}");
}

#[test]
fn counts_cover_what_analysis_makes_of_every_fixture() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let mut checked = 0usize;
    for path in collect_fixture_files(&root).unwrap() {
        let relative = path.strip_prefix(&root).unwrap();
        let is_source = matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau"));
        if !is_source || is_error_fixture(relative) {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        assert_counts_cover_the_tables(&source, detect_version(relative));
        checked = checked.saturating_add(1);
    }
    assert!(checked > 0, "no fixtures found under {}", root.display());
}

#[test]
fn counts_cover_what_analysis_makes_of_generated_programs() {
    for version in LuaVersion::ALL {
        for seed in 0..40 {
            assert_counts_cover_the_tables(&generate_full(seed, version, 25), version);
            assert_counts_cover_the_tables(&generate_runtime_safe(seed, version, 25), version);
        }
    }
}
