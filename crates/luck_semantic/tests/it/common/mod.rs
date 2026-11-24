use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use luck_testgen::{
    collect_fixture_files, detect_version, generate_full, generate_runtime_safe, is_error_fixture,
};
use luck_token::LuaVersion;

/// Runs `check` on every fixture that isn't an error fixture, as the
/// version its path names, and on generated programs of every version.
pub(crate) fn for_each_valid_source(mut check: impl FnMut(&str, LuaVersion)) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let mut checked = 0usize;
    for path in collect_fixture_files(&root).unwrap() {
        let relative = path.strip_prefix(&root).unwrap();
        let is_source = matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau"));
        if !is_source || is_error_fixture(relative) {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        check(&source, detect_version(relative));
        checked += 1;
    }
    assert!(checked > 0, "no fixtures found under {}", root.display());
    for version in LuaVersion::ALL {
        for seed in 0..40 {
            check(&generate_full(seed, version, 25), version);
            check(&generate_runtime_safe(seed, version, 25), version);
        }
    }
}
