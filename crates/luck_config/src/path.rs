use std::path::{self, Component, Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use super::*;

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
