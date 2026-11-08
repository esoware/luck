//! Deciding which files belong to a project from `include`/`exclude` globs.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

/// Decides whether a file belongs to the project from `include` and `exclude`
/// globs, matched relative to the config directory. Exclude wins.
///
/// The `Default` filter includes nothing, while [`ProjectFilter::new`] with no
/// `include` takes every `.lua` and `.luau` file.
#[derive(Debug, Clone, Default)]
pub struct ProjectFilter {
    root: PathBuf,
    include: GlobSet,
    exclude: GlobSet,
}

impl ProjectFilter {
    /// # Errors
    ///
    /// Returns a message naming the first invalid glob pattern.
    pub fn new(
        root: &Path,
        include: Option<&[String]>,
        exclude: Option<&[String]>,
    ) -> Result<Self, String> {
        let default_include = ["{**/,}*.lua".to_owned(), "{**/,}*.luau".to_owned()];
        // `is_included` canonicalizes candidates (`\\?\C:\...` on Windows), so
        // a non-canonical root would make `strip_prefix` fail and every glob
        // would see the absolute path.
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        Ok(Self {
            root: canonical_root,
            include: build_set(include.unwrap_or(&default_include))?,
            exclude: build_set(exclude.unwrap_or_default())?,
        })
    }

    #[must_use]
    pub fn is_included(&self, path: &Path) -> bool {
        let canonical = path.canonicalize().ok();
        let candidate = canonical.as_deref().unwrap_or(path);
        let relative = candidate.strip_prefix(&self.root).unwrap_or(candidate);
        self.include.is_match(relative) && !self.exclude.is_match(relative)
    }
}

/// # Errors
///
/// Returns a message naming the first invalid glob pattern.
fn build_set(patterns: &[String]) -> Result<GlobSet, String> {
    // Invalid patterns are hard errors, like unknown config keys, so a typo'd
    // exclude can't quietly let generated code through.
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern)
            .map_err(|error| format!("invalid glob pattern `{pattern}`: {error}"))?;
        builder.add(glob);
    }
    builder.build().map_err(|error| format!("invalid glob set: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_filter_includes_and_excludes() {
        let root = Path::new("/proj");
        let filter = ProjectFilter::new(
            root,
            Some(&["src/**/*.lua".into(), "src/**/*.luau".into()]),
            Some(&["**/gen/**".into()]),
        )
        .expect("valid globs");
        assert!(filter.is_included(Path::new("/proj/src/a.lua")));
        assert!(!filter.is_included(Path::new("/proj/src/gen/b.lua")));
        assert!(!filter.is_included(Path::new("/proj/other/c.lua")));
    }

    #[test]
    fn project_filter_default_includes_lua_luau() {
        let filter = ProjectFilter::new(Path::new("/proj"), None, None).expect("valid globs");
        assert!(filter.is_included(Path::new("/proj/x.lua")));
        assert!(filter.is_included(Path::new("/proj/sub/y.luau")));
        assert!(!filter.is_included(Path::new("/proj/x.txt")));
    }
}
