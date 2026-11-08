//! The forward-slash path form resolved modules are keyed on, and the way
//! back to a native path.

use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

/// Canonicalizes `path` when it exists, converts it to forward slashes, and
/// drops the Windows extended-length prefix.
///
/// The bundler keys modules on this form, and it displays the same on every
/// platform.
#[must_use]
pub fn normalize_path(path: &Path) -> String {
    let canonical = path.canonicalize();
    let slashed = canonical.as_deref().unwrap_or(path).to_string_lossy().replace('\\', "/");
    // Windows extended-length prefixes after backslash replacement:
    //   \\?\C:\x        -> //?/C:/x        -> C:/x
    //   \\?\UNC\srv\sh  -> //?/UNC/srv/sh  -> //srv/sh (network path!)
    // Stripping `//?/` alone would turn the network path into the relative
    // `UNC/srv/sh`.
    if let Some(unc) = slashed.strip_prefix("//?/UNC/") {
        return format!("//{unc}");
    }
    if let Some(local) = slashed.strip_prefix("//?/") {
        return local.to_owned();
    }
    slashed
}

/// The native form of a forward-slash path, such as a [`normalize_path`]
/// result or a require string, for filesystem calls.
#[must_use]
pub fn native_path(path: &str) -> PathBuf {
    PathBuf::from(path.replace('/', MAIN_SEPARATOR_STR))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn normalize_path_gives_an_absolute_forward_slash_path_for_an_existing_file() {
        let directory = tempfile::tempdir().expect("create temp dir");
        let source_directory = directory.path().join("src");
        fs::create_dir_all(&source_directory).expect("create parent directories");
        fs::write(source_directory.join("main.lua"), "return {}").expect("write file");
        let normalized = normalize_path(&source_directory.join("main.lua"));
        assert!(!normalized.contains('\\'), "{normalized}");
        assert!(!normalized.starts_with("//?/"), "{normalized}");
        assert!(Path::new(&normalized).is_absolute(), "{normalized}");
        assert!(normalized.ends_with("/src/main.lua"), "{normalized}");
    }
}
