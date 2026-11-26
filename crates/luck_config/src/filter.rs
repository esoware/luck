use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::{Candidate, GlobSet};
use luck_resolver::FileSystem;

use crate::glob::{Glob, compiled, glob_set};

/// `path` under its directory's canonical path in `file_system`, with its own
/// name, so a symlink stays where it is and doesn't move to where it leads.
///
/// A file takes the project of its directory, and its `include`, `exclude`,
/// and `overrides` globs match this, so all of them go by the same path.
pub fn located_path(file_system: &dyn FileSystem, path: &Path) -> PathBuf {
    let directory = match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Path::new("."),
        Some(parent) => parent,
        None => return file_system.canonicalize_existing(path),
    };
    match path.file_name() {
        Some(name) => file_system.canonicalize_existing(directory).join(name),
        None => file_system.canonicalize_existing(path),
    }
}

/// Decides whether a file belongs to the project from `include` and `exclude`
/// globs, matched relative to the config directory. Exclude wins.
#[derive(Debug, Clone)]
pub struct ProjectFilter {
    file_system: Arc<dyn FileSystem>,
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
    pub(crate) fn everything(root: &Path, file_system: Arc<dyn FileSystem>) -> Self {
        Self {
            root: file_system.canonicalize_existing(root),
            file_system,
            include: None,
            exclude: GlobSet::empty(),
            excluded_trees: GlobSet::empty(),
        }
    }

    pub(crate) fn new(
        root: &Path,
        include: Option<&[Glob]>,
        exclude: &[Glob],
        file_system: Arc<dyn FileSystem>,
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
            root: file_system.canonicalize_existing(root),
            file_system,
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
            && self.excludes_canonical_tree(&self.file_system.canonicalize_existing(directory))
    }

    /// [`excludes_tree`](Self::excludes_tree) for a canonical path, such as a
    /// walk from a canonical directory builds, which touches no disk.
    pub fn excludes_canonical_tree(&self, directory: &Path) -> bool {
        !self.excluded_trees.is_empty() && self.excluded_trees.is_match(self.relative(directory))
    }

    pub fn is_included(&self, path: &Path) -> bool {
        self.includes_located(&located_path(&*self.file_system, path))
    }

    /// [`is_included`](Self::is_included) for a path [`located_path`] gives,
    /// or a walk from a canonical directory builds, which touches no disk.
    pub fn includes_located(&self, path: &Path) -> bool {
        (self.include.is_some() || is_source_file(path)) && self.includes_document(path)
    }

    /// [`includes_located`](Self::includes_located) for a document an editor
    /// opened as Luau, which with no `include` the project takes whatever its
    /// name, as an editor's `inmemory://model/1` has none of Luau's.
    pub fn includes_document(&self, path: &Path) -> bool {
        let candidate = Candidate::new(self.relative(path));
        self.include.as_ref().is_none_or(|include| include.is_match_candidate(&candidate))
            && (self.exclude.is_empty() || !self.exclude.is_match_candidate(&candidate))
    }

    /// Whether an `exclude` glob matches `path`, whatever `include` says. A
    /// file named on the command line is held to this alone.
    pub fn is_excluded(&self, path: &Path) -> bool {
        let located = located_path(&*self.file_system, path);
        !self.exclude.is_empty() && self.exclude.is_match(self.relative(&located))
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
    use luck_resolver::FileSystemOs;

    use super::*;
    use crate::fixtures::glob;

    fn globs(patterns: &[&str]) -> Vec<Glob> {
        patterns.iter().map(|pattern| glob(pattern).expect("valid glob")).collect()
    }

    fn filter(include: Option<&[&str]>, exclude: &[&str]) -> ProjectFilter {
        let include_globs = include.map(globs);
        let root = Path::new("/proj");
        ProjectFilter::new(root, include_globs.as_deref(), &globs(exclude), Arc::new(FileSystemOs))
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
        let filter = ProjectFilter::everything(Path::new("/proj"), Arc::new(FileSystemOs));
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
