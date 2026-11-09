//! The forward-slash path form resolved modules are keyed on, and the way
//! back to a native path.

use std::path::{self, Component, MAIN_SEPARATOR_STR, Path, PathBuf};

/// Canonicalizes `path` when it exists, converts it to forward slashes, and
/// drops the Windows extended-length prefix where the result stays absolute.
///
/// The bundler keys modules on this form, and it displays the same on every
/// platform.
#[must_use]
pub fn normalize_path(path: &Path) -> String {
    let canonical = path.canonicalize();
    let slashed = canonical.as_deref().unwrap_or(path).to_string_lossy().replace('\\', "/");
    // Windows extended-length prefixes after backslash replacement:
    //   \\?\C:\x          -> //?/C:/x          -> C:/x
    //   \\?\UNC\srv\sh    -> //?/UNC/srv/sh    -> //srv/sh (network path!)
    //   \\?\Volume{id}\x  -> //?/Volume{id}/x  -> kept, nothing shorter is absolute
    if let Some(unc) = slashed.strip_prefix("//?/UNC/") {
        return format!("//{unc}");
    }
    if let Some(local) = slashed.strip_prefix("//?/")
        && local.as_bytes().get(1) == Some(&b':')
    {
        return local.to_owned();
    }
    slashed
}

/// Canonicalizes `path` as far as it exists and resolves the rest lexically,
/// so a file not on disk yet compares against real paths.
pub(crate) fn canonicalize_existing(path: &Path) -> PathBuf {
    let absolute = path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut missing = Vec::new();
    for ancestor in absolute.ancestors() {
        if let Ok(mut canonical) = ancestor.canonicalize() {
            for component in missing.into_iter().rev() {
                match component {
                    Component::CurDir => {}
                    Component::ParentDir => {
                        canonical.pop();
                    }
                    named @ (Component::Normal(_) | Component::Prefix(_) | Component::RootDir) => {
                        canonical.push(named);
                    }
                }
            }
            return canonical;
        }
        missing.extend(ancestor.components().next_back());
    }
    absolute
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

    #[test]
    fn canonicalize_existing_resolves_only_the_part_that_is_missing() {
        let directory = tempfile::tempdir().expect("create temp dir");
        let canonical_root = directory.path().canonicalize().expect("canonicalize root");
        assert_eq!(
            canonicalize_existing(&directory.path().join("gone/../out/bundle.lua")),
            canonical_root.join("out").join("bundle.lua")
        );
    }
}
