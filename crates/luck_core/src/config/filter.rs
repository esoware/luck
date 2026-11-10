use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::path::canonicalize_existing;

/// How deeply a glob's `{` groups may nest. `globset` recurses once per level
/// while it builds, and no real pattern comes close.
const MAX_BRACE_DEPTH: usize = 64;

/// Decides whether a file belongs to the project from `include` and `exclude`
/// globs, matched relative to the config directory. `*` stops at `/` and `**/`
/// crosses directories. Exclude wins.
///
/// The `Default` filter includes nothing, while [`ProjectFilter::new`] with no
/// `include` takes every `.lua` and `.luau` file.
#[derive(Debug, Clone, Default)]
pub struct ProjectFilter {
    root: PathBuf,
    include: GlobSet,
    exclude: GlobSet,
    /// The directory part of each exclude ending in `/**` or `/**/*`, every
    /// descendant of which that exclude covers.
    excluded_trees: GlobSet,
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
        let exclude_patterns = exclude.unwrap_or_default();
        let excluded_trees: Vec<String> = exclude_patterns
            .iter()
            .filter_map(|pattern| {
                pattern.strip_suffix("/**").or_else(|| pattern.strip_suffix("/**/*"))
            })
            .filter(|directory| !directory.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(Self {
            // `is_included` canonicalizes candidates (`\\?\C:\...` on Windows),
            // so a non-canonical root would make `strip_prefix` fail and every
            // glob would see the absolute path.
            root: canonicalize_existing(root),
            include: build_set(include.unwrap_or(&default_include))?,
            exclude: build_set(exclude_patterns)?,
            // A directory part that doesn't compile alone only costs the pruning.
            excluded_trees: build_set(&excluded_trees).unwrap_or_default(),
        })
    }

    /// Whether an exclude covers everything under `directory`, so a walk
    /// needn't enter it.
    #[must_use]
    pub fn excludes_tree(&self, directory: &Path) -> bool {
        if self.excluded_trees.is_empty() {
            return false;
        }
        let candidate = canonicalize_existing(directory);
        let relative = candidate.strip_prefix(&self.root).unwrap_or(&candidate);
        self.excluded_trees.is_match(relative)
    }

    #[must_use]
    pub fn is_included(&self, path: &Path) -> bool {
        let candidate = canonicalize_existing(path);
        let relative = candidate.strip_prefix(&self.root).unwrap_or(&candidate);
        self.include.is_match(relative) && !self.exclude.is_match(relative)
    }

    /// Whether an `exclude` glob matches `path`, whatever `include` says. A
    /// file named on the command line is held to this alone.
    #[must_use]
    pub fn is_excluded(&self, path: &Path) -> bool {
        let candidate = canonicalize_existing(path);
        let relative = candidate.strip_prefix(&self.root).unwrap_or(&candidate);
        self.exclude.is_match(relative)
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
        if brace_depth(pattern) > MAX_BRACE_DEPTH {
            return Err(format!(
                "invalid glob pattern `{pattern}`: `{{` groups nest more than {MAX_BRACE_DEPTH} \
                 levels deep"
            ));
        }
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| format!("invalid glob pattern `{pattern}`: {e}"))?;
        builder.add(glob);
    }
    builder.build().map_err(|e| format!("invalid glob set: {e}"))
}

fn brace_depth(pattern: &str) -> usize {
    let mut depth: usize = 0;
    let mut deepest = 0;
    for b in pattern.bytes() {
        match b {
            b'{' => {
                depth = depth.saturating_add(1);
                deepest = deepest.max(depth);
            }
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_included_files_unless_excluded() {
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
    fn is_excluded_ignores_include() {
        let filter = ProjectFilter::new(
            Path::new("/proj"),
            Some(&["src/**/*.lua".into()]),
            Some(&["**/gen/**".into()]),
        )
        .expect("valid globs");
        assert!(filter.is_excluded(Path::new("/proj/src/gen/b.lua")));
        assert!(!filter.is_excluded(Path::new("/proj/other/c.lua")));
        assert!(!filter.is_excluded(Path::new("/proj/tool")));
    }

    #[test]
    fn excludes_tree_only_when_a_glob_covers_every_descendant() {
        let filter = ProjectFilter::new(
            Path::new("/proj"),
            None,
            Some(&["gen/**".into(), "**/vendor/**".into(), "dist/**/*".into(), "shallow/*".into()]),
        )
        .expect("valid globs");
        assert!(filter.excludes_tree(Path::new("/proj/gen")));
        assert!(filter.excludes_tree(Path::new("/proj/sub/vendor")));
        assert!(filter.excludes_tree(Path::new("/proj/dist")));
        assert!(!filter.excludes_tree(Path::new("/proj/shallow")));
        assert!(!filter.excludes_tree(Path::new("/proj/other")));
    }

    #[test]
    fn default_include_takes_lua_and_luau_files_at_any_depth() {
        let filter = ProjectFilter::new(Path::new("/proj"), None, None).expect("valid globs");
        assert!(filter.is_included(Path::new("/proj/x.lua")));
        assert!(filter.is_included(Path::new("/proj/sub/y.luau")));
        assert!(!filter.is_included(Path::new("/proj/x.txt")));
    }
}
