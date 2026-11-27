use std::error::Error;
use std::fmt;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use ignore::{WalkBuilder, WalkState};
use luck_resolver::{FileSystem as _, FileSystemOs};
use parking_lot::Mutex;

use crate::filter::ProjectFilter;

const LUCKIGNORE_FILE_NAME: &str = ".luckignore";

/// A directory a walk starts at, and its canonical path, which every path the
/// walk reaches under it maps onto without asking the disk.
#[derive(Debug, Clone, Copy)]
pub struct WalkRoot<'path> {
    pub path: &'path Path,
    pub canonical: &'path Path,
}

/// A directory or a source file a walk reached.
#[derive(Debug, Clone, Copy)]
pub struct WalkEntry<'walk> {
    /// Under the root the walk reached it from, as that root is spelled.
    pub path: &'walk Path,
    /// How many levels below its root it is.
    pub depth: usize,
}

/// A source file a walk took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkedFile {
    /// Under the root the walk reached it from, as that root is spelled.
    pub path: PathBuf,
    /// Of a symlink, where it leads.
    pub canonical: PathBuf,
}

/// An entry a walk couldn't read.
#[derive(Debug)]
pub struct WalkError(ignore::Error);

impl fmt::Display for WalkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Error for WalkError {}

/// What [`walk_source_files`] asks of its caller: which project each directory
/// and source file it reaches is in, and whether to go on.
pub trait WalkVisitor: Sync {
    /// What ends the walk at an entry it can't read.
    type Error: Send;

    /// The filter of the project `directory` is in, which leaves it and
    /// everything under it out when it excludes the whole tree, or `None` to
    /// go into it unasked.
    fn directory_filter(
        &self,
        directory: &WalkEntry<'_>,
    ) -> Option<impl Deref<Target = ProjectFilter>>;

    /// The filter of the project `file` is in, which takes the file when it
    /// includes it.
    fn file_filter(&self, file: &WalkEntry<'_>) -> impl Deref<Target = ProjectFilter>;

    /// Whether the walk goes into `directory`, which its filter keeps.
    fn enters(&self, _directory: &WalkEntry<'_>) -> bool {
        true
    }

    /// Whether the walk stops before its next entry.
    fn is_stopping(&self) -> bool {
        false
    }

    /// What `error` does to the walk.
    ///
    /// # Errors
    ///
    /// Returns what ends the walk there, or else the walk passes over the
    /// entry.
    fn read_error(&self, error: WalkError) -> Result<(), Self::Error>;
}

/// The source files under `roots`, at most `max_depth` levels down, that their
/// projects take, past what `.gitignore` and `.luckignore` hide, in the order
/// the walk reached them.
///
/// A source file is a file whose extension is `lua` or `luau`, or a symlink with
/// one that leads to a file. The walk follows no symlink, so only a symlink's
/// target needs the disk to place, and its project's globs match the symlink
/// where it is, as [`located_path`](crate::located_path) spells it.
///
/// # Errors
///
/// Returns what `visitor` makes of the first entry that ends the walk.
pub fn walk_source_files<Visitor>(
    roots: &[WalkRoot<'_>],
    max_depth: Option<usize>,
    visitor: &Visitor,
) -> Result<Vec<WalkedFile>, Visitor::Error>
where
    Visitor: WalkVisitor,
{
    let Some((first, others)) = roots.split_first() else {
        return Ok(Vec::new());
    };
    let mut builder = WalkBuilder::new(first.path);
    for root in others {
        builder.add(root.path);
    }
    let found = Mutex::new(Vec::new());
    let ended = Mutex::new(None);
    builder
        .add_custom_ignore_filename(LUCKIGNORE_FILE_NAME)
        .max_depth(max_depth)
        // Measured in `luck lint`: one thread of the parallel walker reads a
        // tree in half the time the serial walker takes, and each thread more
        // idles a millisecond or so before the walk can end, which a small
        // tree pays in full.
        .threads(1)
        .build_parallel()
        .run(|| {
            Box::new(|walked| {
                let entry = match walked {
                    Ok(entry) => entry,
                    Err(error) => {
                        return match visitor.read_error(WalkError(error)) {
                            Ok(()) => WalkState::Continue,
                            Err(end) => {
                                ended.lock().get_or_insert(end);
                                WalkState::Quit
                            }
                        };
                    }
                };
                if visitor.is_stopping() {
                    return WalkState::Quit;
                }
                let Some(file_type) = entry.file_type() else {
                    return WalkState::Continue;
                };
                let path = entry.path();
                let depth = entry.depth();
                if file_type.is_dir() {
                    let canonical = canonical_under(roots, path);
                    let directory = WalkEntry { path, depth };
                    let is_excluded = visitor
                        .directory_filter(&directory)
                        .is_some_and(|filter| filter.excludes_tree(&canonical));
                    return if is_excluded || !visitor.enters(&directory) {
                        WalkState::Skip
                    } else {
                        WalkState::Continue
                    };
                }
                if !path
                    .extension()
                    .is_some_and(|extension| extension == "luau" || extension == "lua")
                {
                    return WalkState::Continue;
                }
                let is_symlink = entry.path_is_symlink();
                if !is_symlink && !file_type.is_file() {
                    return WalkState::Continue;
                }
                let located = canonical_under(roots, path);
                if !visitor.file_filter(&WalkEntry { path, depth }).includes_located(&located) {
                    return WalkState::Continue;
                }
                let canonical = if is_symlink {
                    let Some(target) =
                        FileSystemOs.canonicalize(path).ok().filter(|target| target.is_file())
                    else {
                        return WalkState::Continue;
                    };
                    target
                } else {
                    located
                };
                found.lock().push(WalkedFile { path: entry.into_path(), canonical });
                WalkState::Continue
            })
        });
    match ended.into_inner() {
        Some(end) => Err(end),
        None => Ok(found.into_inner()),
    }
}

/// `path`, which a walk reached under one of `roots`, under that root's
/// canonical path.
fn canonical_under(roots: &[WalkRoot<'_>], path: &Path) -> PathBuf {
    roots
        .iter()
        .find_map(|root| match path.strip_prefix(root.path).ok()? {
            below if below.as_os_str().is_empty() => Some(root.canonical.to_path_buf()),
            below => Some(root.canonical.join(below)),
        })
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink as symlink_file;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_file;

    use luck_resolver::to_slash_lossy;

    use super::*;
    use crate::fixtures::project_with;
    use crate::project::Project;

    /// Every entry in one project, entered no deeper than `max_entered_depth`.
    struct OneProject {
        project: Project,
        max_entered_depth: usize,
    }

    impl WalkVisitor for OneProject {
        type Error = Infallible;

        fn directory_filter(
            &self,
            _: &WalkEntry<'_>,
        ) -> Option<impl Deref<Target = ProjectFilter>> {
            Some(self.project.filter())
        }

        fn file_filter(&self, _: &WalkEntry<'_>) -> impl Deref<Target = ProjectFilter> {
            self.project.filter()
        }

        fn enters(&self, directory: &WalkEntry<'_>) -> bool {
            directory.depth <= self.max_entered_depth
        }

        fn read_error(&self, _: WalkError) -> Result<(), Infallible> {
            Ok(())
        }
    }

    /// The files a walk of a project holding `files` takes, relative to it.
    fn walked(files: &[(&str, &str)], max_entered_depth: usize) -> Vec<String> {
        let directory = project_with(files);
        let project =
            Project::load(&directory.path().join("luck.json"), FileSystemOs).expect("config loads");
        let canonical = FileSystemOs.canonicalize(directory.path()).expect("the project exists");
        let root = WalkRoot { path: directory.path(), canonical: &canonical };
        let Ok(found) =
            walk_source_files(&[root], None, &OneProject { project, max_entered_depth });
        let mut relative: Vec<String> = found
            .iter()
            .map(|file| {
                let below = file.path.strip_prefix(directory.path()).expect("under the root");
                assert_eq!(file.canonical, canonical.join(below));
                to_slash_lossy(below).into_owned()
            })
            .collect();
        relative.sort_unstable();
        relative
    }

    #[test]
    fn takes_the_source_files_the_project_includes_past_luckignore() {
        let files = [
            ("luck.json", r#"{ "exclude": ["gen/**"] }"#),
            (".luckignore", "ignored.luau\n"),
            ("a.lua", ""),
            ("sub/b.luau", ""),
            ("c.txt", ""),
            ("sub/.lua", ""),
            ("ignored.luau", ""),
            ("gen/d.luau", ""),
        ];
        assert_eq!(walked(&files, usize::MAX), ["a.lua", "sub/b.luau"]);
    }

    #[test]
    fn leaves_out_a_directory_its_visitor_does_not_enter() {
        let files =
            [("luck.json", "{}"), ("a.luau", ""), ("sub/b.luau", ""), ("sub/deep/c.luau", "")];
        assert_eq!(walked(&files, 1), ["a.luau", "sub/b.luau"]);
    }

    /// A symlink is taken as its project's globs see it where it is, and read
    /// where it leads.
    #[test]
    fn takes_a_symlink_its_project_includes_where_it_is() {
        let directory =
            project_with(&[("luck.json", r#"{ "include": ["src/**"] }"#), ("src/a.luau", "")]);
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let target = elsewhere.path().join("b.luau");
        fs::write(&target, "").expect("write b.luau");
        // Windows lets only an administrator or developer mode make one.
        if symlink_file(&target, directory.path().join("src/b.luau")).is_err() {
            return;
        }
        let project =
            Project::load(&directory.path().join("luck.json"), FileSystemOs).expect("config loads");
        let canonical = FileSystemOs.canonicalize(directory.path()).expect("the project exists");
        let root = WalkRoot { path: directory.path(), canonical: &canonical };
        let visitor = OneProject { project, max_entered_depth: usize::MAX };
        let Ok(found) = walk_source_files(&[root], None, &visitor);
        let mut taken: Vec<(String, PathBuf)> = found
            .into_iter()
            .map(|file| {
                let below = file.path.strip_prefix(directory.path()).expect("under the root");
                (to_slash_lossy(below).into_owned(), file.canonical)
            })
            .collect();
        taken.sort_unstable();
        let target = FileSystemOs.canonicalize(&target).expect("the target exists");
        assert_eq!(
            taken,
            [
                ("src/a.luau".to_owned(), canonical.join("src").join("a.luau")),
                ("src/b.luau".to_owned(), target)
            ]
        );
    }
}
