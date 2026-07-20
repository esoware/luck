//! Deciding which files belong to a project from `include`/`exclude` globs.

use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::{Path, PathBuf};

/// Decides whether a file is part of the project, from include/exclude
/// globs relative to the config directory. Exclude wins over include.
#[derive(Debug, Clone)]
pub struct ProjectFilter {
    base: PathBuf,
    include: GlobSet,
    exclude: GlobSet,
}

impl ProjectFilter {
    pub fn new(
        base: &Path,
        include: &Option<Vec<String>>,
        exclude: &Option<Vec<String>>,
    ) -> Result<Self, String> {
        // `{**/,}` matches files at the project root and in any subdirectory,
        // since `**/` alone does not match zero path segments.
        let default_include = vec!["{**/,}*.lua".to_string(), "{**/,}*.luau".to_string()];
        let inc = include.clone().unwrap_or(default_include);
        // Canonicalize once: callers canonicalize candidate files (on
        // Windows that yields `\\?\C:\...`), and strip_prefix against a
        // non-canonical base silently fails - every include matched
        // nothing and every exclude excluded nothing.
        let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
        Ok(Self {
            base,
            include: build_set(&inc)?,
            exclude: build_set(exclude.as_deref().unwrap_or(&[]))?,
        })
    }

    /// True if `path` is inside the project and matches include but not exclude.
    pub fn is_included(&self, path: &Path) -> bool {
        let canonical;
        let candidate = match path.canonicalize() {
            Ok(resolved) => {
                canonical = resolved;
                canonical.as_path()
            }
            Err(_) => path,
        };
        let rel = candidate.strip_prefix(&self.base).unwrap_or(candidate);
        self.include.is_match(rel) && !self.exclude.is_match(rel)
    }
}

fn build_set(patterns: &[String]) -> Result<GlobSet, String> {
    // Invalid patterns are hard errors, matching the config contract
    // everywhere else - a typo'd exclude silently processing generated
    // code is exactly the failure deny_unknown_fields exists to prevent.
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob =
            Glob::new(pattern).map_err(|e| format!("invalid glob pattern \"{pattern}\": {e}"))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|e| format!("invalid glob set: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_filter_includes_and_excludes() {
        let base = Path::new("/proj");
        let filter = ProjectFilter::new(
            base,
            &Some(vec!["src/**/*.lua".into(), "src/**/*.luau".into()]),
            &Some(vec!["**/gen/**".into()]),
        )
        .expect("valid globs");
        assert!(filter.is_included(Path::new("/proj/src/a.lua")));
        assert!(!filter.is_included(Path::new("/proj/src/gen/b.lua")));
        assert!(!filter.is_included(Path::new("/proj/other/c.lua")));
    }

    #[test]
    fn project_filter_default_includes_lua_luau() {
        let filter = ProjectFilter::new(Path::new("/proj"), &None, &None).expect("valid globs");
        assert!(filter.is_included(Path::new("/proj/x.lua")));
        assert!(filter.is_included(Path::new("/proj/sub/y.luau")));
        assert!(!filter.is_included(Path::new("/proj/x.txt")));
    }
}
