#![cfg(windows)]

use std::path::Path;

use luck_core::normalize_path;

#[test]
fn bug_normalize_path_keeps_a_volume_guid_path_absolute() {
    let volume_path = r"\\?\Volume{12345678-1234-1234-1234-123456789abc}\src\main.lua";
    let normalized = normalize_path(Path::new(volume_path));
    assert!(
        Path::new(&normalized).is_absolute(),
        "dropping `\\\\?\\` must not turn a volume path relative: {normalized}"
    );
}
