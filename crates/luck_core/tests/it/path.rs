#![cfg(windows)]

use std::ffi::OsString;
use std::fs;
use std::os::windows::ffi::OsStringExt as _;
use std::path::Path;

use luck_core::{native_path, normalize_path};
use tempfile::TempDir;

#[test]
fn bug_normalize_path_keeps_a_volume_guid_path_absolute() {
    let volume_path = r"\\?\Volume{12345678-1234-1234-1234-123456789abc}\src\main.lua";
    let normalized = normalize_path(Path::new(volume_path));
    assert!(
        Path::new(&normalized).is_absolute(),
        "dropping `\\\\?\\` must not turn a volume path relative: {normalized}"
    );
}

#[test]
fn bug_normalize_path_round_trips_a_non_utf8_file_name() {
    let project = TempDir::new().expect("create temp dir");
    let lone_surrogate_name = OsString::from_wide(&[0x61, 0xD800, 0x2E, 0x6C, 0x75, 0x61]);
    let file_path = project.path().join(lone_surrogate_name);
    fs::write(&file_path, "return 1").expect("write file");
    let round_tripped = native_path(&normalize_path(&file_path));
    assert!(
        round_tripped.exists(),
        "`native_path` must lead back to the file `normalize_path` was given: {round_tripped:?}"
    );
}
