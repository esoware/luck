use std::path::{self, Path, PathBuf};

use globset::{Candidate, GlobSet};
use luck_resolver::{FileSystem as _, FileSystemOs};

use crate::glob::{Glob, compiled, glob_set};

/// `path` under its directory's canonical path, with its own name, so a
/// symlink stays where it is and doesn't move to where it leads.
///
/// A file takes the project of its directory, and its `include`, `exclude`,
/// and `overrides` globs match this, so all of them go by the same path.
pub fn located_path(path: &Path) -> PathBuf {
    let absolute = path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match (absolute.parent(), absolute.file_name()) {
        (Some(directory), Some(name)) => FileSystemOs.canonicalize_existing(directory).join(name),
        _ => FileSystemOs.canonicalize_existing(&absolute),
    }
}

/// Decides whether a file belongs to the project from `include` and `exclude`
/// globs, matched relative to the config directory. Exclude wins.
#[derive(Debug, Clone)]
pub struct ProjectFilter {
    /// Canonical, as the paths matched against it are.
    root: PathBuf,
    /// `None` takes every `.luau` and `.lua` file.
    include: Option<GlobSet>,
    exclude: GlobSet,
    /// The directory part of each exclude ending in `/**` or `/**/*`, every
    /// descendant of which that exclude covers.
    excluded_trees: GlobSet,
}

impl ProjectFilter {
    /// Every `.luau` and `.lua` file.
    pub(crate) fn everything(root: &Path) -> Self {
        Self {
            root: FileSystemOs.canonicalize_existing(root),
            include: None,
            exclude: GlobSet::empty(),
            excluded_trees: GlobSet::empty(),
        }
    }

    pub(crate) fn new(
        root: &Path,
        include: Option<&[Glob]>,
        exclude: &[Glob],
    ) -> Result<Self, String> {
        let excluded_trees: Vec<String> = exclude
            .iter()
            .filter_map(|glob| {
                let pattern = glob.pattern();
                pattern.strip_suffix("/**").or_else(|| pattern.strip_suffix("/**/*"))
            })
            .filter(|directory| !directory.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(Self {
            // The paths matched against it are canonical, so a root that
            // isn't would make `strip_prefix` fail and every glob see the
            // absolute path.
            root: FileSystemOs.canonicalize_existing(root),
            include: include.map(compiled).transpose()?,
            exclude: compiled(exclude)?,
            // A directory part that doesn't compile alone only costs the pruning.
            excluded_trees: glob_set(&excluded_trees).unwrap_or_default(),
        })
    }

    /// Whether an exclude covers everything under `directory`, so a walk
    /// needn't enter it.
    pub fn excludes_tree(&self, directory: &Path) -> bool {
        !self.excluded_trees.is_empty()
            && self.excludes_canonical_tree(&FileSystemOs.canonicalize_existing(directory))
    }

    /// [`excludes_tree`](Self::excludes_tree) for a canonical path, such as a
    /// walk from a canonical directory builds, which touches no disk.
    pub fn excludes_canonical_tree(&self, directory: &Path) -> bool {
        !self.excluded_trees.is_empty() && self.excluded_trees.is_match(self.relative(directory))
    }

    pub fn is_included(&self, path: &Path) -> bool {
        self.includes_located(&located_path(path))
    }

    /// [`is_included`](Self::is_included) for a path [`located_path`] gives,
    /// or a walk from a canonical directory builds, which touches no disk.
    pub fn includes_located(&self, path: &Path) -> bool {
        let candidate = Candidate::new(self.relative(path));
        let is_taken = self
            .include
            .as_ref()
            .map_or_else(|| is_source_file(path), |include| include.is_match_candidate(&candidate));
        is_taken && (self.exclude.is_empty() || !self.exclude.is_match_candidate(&candidate))
    }

    /// Whether an `exclude` glob matches `path`, whatever `include` says. A
    /// file named on the command line is held to this alone.
    pub fn is_excluded(&self, path: &Path) -> bool {
        !self.exclude.is_empty() && self.exclude.is_match(self.relative(&located_path(path)))
    }

    /// `located`, a path under canonical directories, as the globs see it,
    /// relative to the root when it's under it.
    pub(crate) fn relative<'path>(&self, located: &'path Path) -> &'path Path {
        located.strip_prefix(&self.root).unwrap_or(located)
    }
}

/// What `**/*.luau` and `**/*.lua` match, without a glob.
fn is_source_file(path: &Path) -> bool {
    path.file_name().is_some_and(|file_name| {
        let name = file_name.as_encoded_bytes();
        name.ends_with(b".luau") || name.ends_with(b".lua")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::glob;

    fn globs(patterns: &[&str]) -> Vec<Glob> {
        patterns.iter().map(|pattern| glob(pattern).expect("valid glob")).collect()
    }

    fn filter(include: Option<&[&str]>, exclude: &[&str]) -> ProjectFilter {
        let include_globs = include.map(globs);
        ProjectFilter::new(Path::new("/proj"), include_globs.as_deref(), &globs(exclude))
            .expect("valid globs")
    }

    #[test]
    fn takes_included_files_unless_excluded() {
        let filter = filter(Some(&["src/**/*.lua", "src/**/*.luau"]), &["**/gen/**"]);
        assert!(filter.is_included(Path::new("/proj/src/a.luau")));
        assert!(!filter.is_included(Path::new("/proj/src/gen/b.luau")));
        assert!(!filter.is_included(Path::new("/proj/other/c.luau")));
    }

    #[test]
    fn is_excluded_ignores_include() {
        let filter = filter(Some(&["src/**/*.luau"]), &["**/gen/**"]);
        assert!(filter.is_excluded(Path::new("/proj/src/gen/b.luau")));
        assert!(!filter.is_excluded(Path::new("/proj/other/c.luau")));
        assert!(!filter.is_excluded(Path::new("/proj/tool")));
    }

    #[test]
    fn excludes_tree_only_when_a_glob_covers_every_descendant() {
        let filter = filter(None, &["gen/**", "**/vendor/**", "dist/**/*", "shallow/*"]);
        assert!(filter.excludes_tree(Path::new("/proj/gen")));
        assert!(filter.excludes_tree(Path::new("/proj/sub/vendor")));
        assert!(filter.excludes_tree(Path::new("/proj/dist")));
        assert!(!filter.excludes_tree(Path::new("/proj/shallow")));
        assert!(!filter.excludes_tree(Path::new("/proj/other")));
    }

    #[test]
    fn everything_takes_lua_and_luau_files_at_any_depth() {
        let filter = ProjectFilter::everything(Path::new("/proj"));
        assert!(filter.is_included(Path::new("/proj/x.lua")));
        assert!(filter.is_included(Path::new("/proj/sub/y.luau")));
        assert!(filter.is_included(Path::new("/proj/.lua")));
        assert!(filter.is_included(Path::new("/elsewhere/z.luau")));
        assert!(!filter.is_included(Path::new("/proj/x.txt")));
        assert!(!filter.is_included(Path::new("/proj/x.luau.bak")));
        assert!(!filter.is_included(Path::new("/proj/x.luaux")));
    }

    #[test]
    fn no_include_takes_what_everything_takes_less_the_excludes() {
        let filter = filter(None, &["gen/**"]);
        assert!(filter.is_included(Path::new("/proj/a.luau")));
        assert!(!filter.is_included(Path::new("/proj/gen/b.luau")));
        assert!(!filter.is_included(Path::new("/proj/a.txt")));
    }
}
