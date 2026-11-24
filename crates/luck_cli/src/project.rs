use alloc::sync::Arc;
use core::num::NonZero;
use core::panic::AssertUnwindSafe;
use core::{iter, mem};
use std::panic;
use std::path::{Component, MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf};
use std::sync::mpsc;
use std::{env, thread};

use globset::GlobSet;
use ignore::{DirEntry, WalkBuilder, WalkState};
use luck_allocator::{Allocator, AllocatorPool};
use luck_ast::MIN_STACK_BYTES;
use luck_bundler::{BundleOptions, DynamicRequire};
use luck_config::{CONFIG_FILE_NAME, Project, ProjectFilter, find_config, glob_set};
use luck_linter::LintConfig;
use luck_resolver::{FileSystem as _, FileSystemOs};
use luck_token::LuaVersion;
use rayon::ThreadPoolBuilder;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::exit::{Exit, config_error, operation_error, usage_error};

/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] when the working
/// directory can't be read.
pub(crate) fn current_directory() -> Result<PathBuf, Exit> {
    env::current_dir().map_err(|e| usage_error(format!("failed to get current directory: {e}")))
}

/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] when it doesn't.
pub(crate) fn existing_file(input: &str) -> Result<&Path, Exit> {
    let path = Path::new(input);
    if path.is_file() { Ok(path) } else { Err(usage_error(format!("file not found: `{input}`"))) }
}

/// Builds rayon's global pool, which a command that works on many files runs
/// them on, with `threads` threads, each with the stack a deep file needs.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Failure`] when a thread can't
/// start.
pub(crate) fn start_thread_pool(threads: NonZero<usize>) -> Result<(), Exit> {
    ThreadPoolBuilder::new()
        .num_threads(threads.get())
        .stack_size(MIN_STACK_BYTES)
        .build_global()
        .map_err(|e| operation_error(format!("can't start {threads} threads: {e}")))
}

/// How `bundle` and `graph` resolve and read modules: as `project` says, with
/// `-s` replacing its search paths and `--dynamic-require` its mode when
/// given.
pub(crate) fn bundle_options<'project>(
    project: &'project Project,
    version: LuaVersion,
    search_paths: &'project [String],
    dynamic_require: Option<DynamicRequire>,
) -> BundleOptions<'project> {
    let options = project.bundle_options(version);
    BundleOptions {
        search_paths: if search_paths.is_empty() { options.search_paths } else { search_paths },
        dynamic_require: dynamic_require.unwrap_or(options.dynamic_require),
        ..options
    }
}

/// # Errors
///
/// Reports the config's problem and returns [`Exit::Usage`] when it doesn't
/// load.
pub(crate) fn load_project(config_path: &Path) -> Result<Project, Exit> {
    Project::load(config_path).map_err(|error| config_error(&error))
}

/// The project a command that takes one input reads it under: `-c` when
/// given, or else the `luck.json` that governs `input`.
///
/// Discovery starts in the input's own directory, or in the input when it's a
/// directory, not cwd, so `luck bundle path/to/project/src/main.lua` sees that
/// project's config. With none, it's the defaults rooted where discovery
/// started, so a bundle resolves its modules from the entry's directory.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] when the working directory
/// is unavailable or the config doesn't load.
pub(crate) fn one_shot_project(config_path: Option<&Path>, input: &Path) -> Result<Project, Exit> {
    if let Some(path) = config_path {
        return load_project(path);
    }
    let input_directory = if input.is_dir() {
        Some(input)
    } else {
        input.parent().filter(|parent| !parent.as_os_str().is_empty())
    };
    let start_directory = match input_directory {
        Some(directory) => match FileSystemOs.canonicalize(directory) {
            Ok(canonical) => canonical,
            Err(_) => current_directory()?.join(directory),
        },
        // A bare file name, or `-` for stdin: cwd is the only context there is.
        None => current_directory()?,
    };
    find_config(&start_directory)
        .map_or_else(|| Ok(Project::defaults(start_directory)), |found| load_project(&found))
}

/// The project of `-c` when given, or else of the first `luck.json` found
/// walking up from cwd, or else the defaults rooted at cwd.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] when the working directory
/// is unavailable or the config doesn't load.
pub(crate) fn working_project(config_path: Option<&Path>) -> Result<Project, Exit> {
    let working_directory = current_directory()?;
    config_path
        .map(Path::to_path_buf)
        .or_else(|| find_config(&working_directory))
        .map_or_else(|| Ok(Project::defaults(working_directory)), |found| load_project(&found))
}

/// A file a path command works on, under the project that governs it.
#[derive(Debug)]
pub(crate) struct ProjectFile {
    pub path: PathBuf,
    pub project: Arc<Project>,
}

/// The files `paths` select, each under the nearest `luck.json` above it, or
/// all under `config_path` when `-c` gives one. No path means the working
/// directory.
///
/// A directory is walked and gated by its project's include/exclude filter,
/// and a project nested in it is walked under its own config. A glob selects
/// from the walk of its literal directory part, a file named outright skips
/// `include` but not `exclude`, and a `!` glob drops whatever it matches and
/// everything under a directory it matches. A file several arguments reach is
/// processed once.
///
/// Every project gets `lint_overlay` layered over its lint settings. Each
/// loads here, before the parallel commands' locked output sections, which
/// hold the stdio locks on the main thread while rayon workers run, so a
/// worker reporting a config error would deadlock.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] on a config that doesn't
/// load, a bad glob, a path that doesn't exist, or, unless
/// `can_match_nothing`, a glob that selects no file or no file left to work
/// on, and [`Exit::Failure`] when a directory can't be walked.
pub(crate) fn project_files(
    config_path: Option<&Path>,
    paths: &[String],
    can_match_nothing: bool,
    lint_overlay: Option<&LintConfig>,
) -> Result<Vec<ProjectFile>, Exit> {
    let working_directory = current_directory()?;
    let mut projects = Projects::new(config_path, &working_directory, lint_overlay)?;

    let (dropped_args, selected_args): (Vec<&str>, Vec<&str>) =
        paths.iter().map(String::as_str).partition(|arg| arg.starts_with('!'));
    let default_args = ["."];
    let requested_args =
        if selected_args.is_empty() { default_args.as_slice() } else { selected_args.as_slice() };

    let mut found: Vec<ProjectFile> = Vec::new();
    for arg in requested_args {
        let path = Path::new(arg);
        if path.is_dir() {
            projects.collect_tree(path, None, &mut found)?;
        } else if path.is_file() {
            let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
            let project = projects.for_directory(parent.unwrap_or_else(|| Path::new(".")))?;
            if !project.filter().is_excluded(path) {
                found.push(ProjectFile { path: path.to_path_buf(), project });
            }
        } else if is_glob(arg) {
            let (base, pattern) = split_glob(arg);
            let max_depth = (!pattern.contains("**")).then(|| pattern.split('/').count());
            let patterns = glob_set(&[pattern]).map_err(usage_error)?;
            let selection = Selection { patterns, max_depth };
            let found_before = found.len();
            if base.is_dir() {
                projects.collect_tree(&base, Some(&selection), &mut found)?;
            }
            if found.len() == found_before && !can_match_nothing {
                return Err(usage_error(format!("no `.lua` or `.luau` files matched `{arg}`")));
            }
        } else {
            return Err(usage_error(format!("path not found: `{arg}`")));
        }
    }

    if !dropped_args.is_empty() {
        let mut dropped = Vec::with_capacity(dropped_args.len());
        for arg in dropped_args {
            let (base, pattern) = split_glob(arg.trim_start_matches('!'));
            let patterns = if pattern.is_empty() {
                vec!["**".to_owned()]
            } else {
                vec![format!("{pattern}/**"), pattern]
            };
            let absolute_base = lexical_absolute(&working_directory, &base);
            dropped.push((absolute_base, glob_set(&patterns).map_err(usage_error)?));
        }
        found.retain(|file| {
            let absolute = lexical_absolute(&working_directory, &file.path);
            !dropped.iter().any(|(base, patterns)| {
                absolute.strip_prefix(base).is_ok_and(|relative| {
                    relative.as_os_str().is_empty() || patterns.is_match(relative)
                })
            })
        });
    }

    // One argument can't reach a file twice, so only several pay for this.
    if requested_args.len() > 1 {
        let mut seen_paths = FxHashSet::default();
        found.retain(|file| {
            seen_paths
                .insert(FileSystemOs.canonicalize(&file.path).unwrap_or_else(|_| file.path.clone()))
        });
    }

    if found.is_empty() && !can_match_nothing {
        let quoted_args: Vec<String> = paths.iter().map(|arg| format!("`{arg}`")).collect();
        let selection =
            if quoted_args.is_empty() { "`.`".to_owned() } else { quoted_args.join(", ") };
        return Err(usage_error(format!("no `.lua` or `.luau` files matched {selection}")));
    }
    Ok(found)
}

/// Whether `arg`, which names no existing path, is a glob and not a typo.
fn is_glob(arg: &str) -> bool {
    arg.contains(['*', '?', '[', '{'])
}

/// `arg` split before its first segment with a wildcard: the directory that
/// holds everything it can match, and the `/`-separated pattern a path below
/// that directory has to match.
fn split_glob(arg: &str) -> (PathBuf, String) {
    let slashed = arg.replace(MAIN_SEPARATOR, "/");
    let segments: Vec<&str> = slashed.strip_prefix("./").unwrap_or(&slashed).split('/').collect();
    let literal_count = segments.iter().take_while(|segment| !is_glob(segment)).count();
    let (literal, pattern) = segments.split_at(literal_count);
    // The trailing separator keeps `/` and `C:\` roots instead of emptying
    // them.
    let base = if literal.is_empty() {
        ".".to_owned()
    } else {
        format!("{}{MAIN_SEPARATOR}", literal.join(MAIN_SEPARATOR_STR))
    };
    (PathBuf::from(base), pattern.join("/"))
}

/// `path` from `working_directory` with `.` and `..` resolved without touching
/// the disk.
fn lexical_absolute(working_directory: &Path, path: &Path) -> PathBuf {
    let mut absolute = PathBuf::new();
    for component in working_directory.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                absolute.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                absolute.push(component);
            }
        }
    }
    absolute
}

/// What a glob argument selects below its base directory.
struct Selection {
    patterns: GlobSet,
    /// How many levels below the base a match can sit, when the pattern has
    /// no `**`.
    max_depth: Option<usize>,
}

/// The projects a run has met, one per `luck.json`.
struct Projects<'overlay> {
    /// Set by `-c`, which puts every file under that one config.
    explicit: Option<Arc<Project>>,
    /// The defaults, rooted at the working directory, for a path with no
    /// `luck.json` above it.
    fallback: Option<Arc<Project>>,
    working_directory: PathBuf,
    /// By the path of their `luck.json`.
    discovered: FxHashMap<PathBuf, Arc<Project>>,
    by_directory: FxHashMap<PathBuf, Arc<Project>>,
    lint_overlay: Option<&'overlay LintConfig>,
}

impl<'overlay> Projects<'overlay> {
    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Usage`] when `config_path`
    /// doesn't load.
    fn new(
        config_path: Option<&Path>,
        working_directory: &Path,
        lint_overlay: Option<&'overlay LintConfig>,
    ) -> Result<Self, Exit> {
        let mut projects = Self {
            explicit: None,
            fallback: None,
            working_directory: working_directory.to_path_buf(),
            discovered: FxHashMap::default(),
            by_directory: FxHashMap::default(),
            lint_overlay,
        };
        if let Some(path) = config_path {
            projects.explicit = Some(projects.prepared(load_project(path)?));
        }
        Ok(projects)
    }

    fn prepared(&self, mut project: Project) -> Arc<Project> {
        if let Some(overlay) = self.lint_overlay {
            project.overlay_lint(overlay);
        }
        Arc::new(project)
    }

    /// The project of the nearest `luck.json` at or above `directory`.
    ///
    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Usage`] when that config
    /// doesn't load.
    fn for_directory(&mut self, directory: &Path) -> Result<Arc<Project>, Exit> {
        if let Some(explicit) = &self.explicit {
            return Ok(Arc::clone(explicit));
        }
        self.for_canonical_directory(&canonical_directory(directory)?)
    }

    /// [`for_directory`](Self::for_directory) for a canonical `directory`.
    ///
    /// # Errors
    ///
    /// As for [`for_directory`](Self::for_directory).
    fn for_canonical_directory(&mut self, directory: &Path) -> Result<Arc<Project>, Exit> {
        if let Some(explicit) = &self.explicit {
            return Ok(Arc::clone(explicit));
        }
        if let Some(known) = self.by_directory.get(directory) {
            return Ok(Arc::clone(known));
        }
        // Found by stat alone, so a project met before isn't loaded again.
        let project = if let Some(config_path) = find_config(directory) {
            if let Some(known) = self.discovered.get(&config_path) {
                Arc::clone(known)
            } else {
                let discovered = self.prepared(load_project(&config_path)?);
                self.discovered.insert(config_path, Arc::clone(&discovered));
                discovered
            }
        } else if let Some(fallback) = &self.fallback {
            Arc::clone(fallback)
        } else {
            let fallback = self.prepared(Project::defaults(self.working_directory.clone()));
            self.fallback = Some(Arc::clone(&fallback));
            fallback
        };
        self.by_directory.insert(directory.to_path_buf(), Arc::clone(&project));
        Ok(project)
    }

    /// Appends the Lua files under `directory` that their project's filter
    /// takes and `selection` matches relative to `directory`, sorted by path.
    ///
    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Failure`] when part of
    /// `directory` outside any excluded tree can't be walked, since the files
    /// in it would go unprocessed.
    fn collect_tree(
        &mut self,
        directory: &Path,
        selection: Option<&Selection>,
        found: &mut Vec<ProjectFile>,
    ) -> Result<(), Exit> {
        let has_nested_projects = self.explicit.is_none();
        let mut tree_files = Vec::new();
        let max_depth = selection.and_then(|selected| selected.max_depth);
        let mut pending_trees = vec![(directory.to_path_buf(), max_depth)];
        while let Some((tree, tree_max_depth)) = pending_trees.pop() {
            let canonical_tree = canonical_directory(&tree)?;
            let project = self.for_canonical_directory(&canonical_tree)?;
            let walk = TreeWalk {
                tree: &tree,
                canonical_tree: &canonical_tree,
                max_depth: tree_max_depth,
                filter: project.filter(),
                selection: selection.map(|selected| (directory, &selected.patterns)),
                has_nested_projects,
            };
            let walked = walk.run()?;
            tree_files.extend(
                walked
                    .files
                    .into_iter()
                    .map(|path| ProjectFile { path, project: Arc::clone(&project) }),
            );
            pending_trees.extend(walked.nested_trees);
        }
        tree_files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
        found.extend(tree_files);
        Ok(())
    }
}

/// One project's tree, walked for the Lua files its filter takes.
struct TreeWalk<'walk> {
    tree: &'walk Path,
    canonical_tree: &'walk Path,
    max_depth: Option<usize>,
    filter: &'walk ProjectFilter,
    /// The directory a glob argument names, and the patterns a path below it
    /// has to match.
    selection: Option<(&'walk Path, &'walk GlobSet)>,
    has_nested_projects: bool,
}

/// What a walk found, or one of its visitors' share of it.
#[derive(Default)]
struct Walked {
    files: Vec<PathBuf>,
    /// Each with how much deeper a glob without `**` can still match in it.
    nested_trees: Vec<(PathBuf, Option<usize>)>,
    error: Option<ignore::Error>,
}

/// A visitor's share, sent when the walk drops the visitor.
struct WalkerShare {
    walked: Walked,
    sender: mpsc::Sender<Walked>,
}

impl Drop for WalkerShare {
    fn drop(&mut self) {
        let _unused = self.sender.send(mem::take(&mut self.walked));
    }
}

impl TreeWalk<'_> {
    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Failure`] when part of the
    /// tree outside any excluded directory can't be walked.
    fn run(&self) -> Result<Walked, Exit> {
        let (sender, receiver) = mpsc::channel();
        WalkBuilder::new(self.tree)
            .add_custom_ignore_filename(".luckignore")
            .max_depth(self.max_depth)
            // Measured: one thread of the parallel walker reads a tree in half
            // the time the serial walker takes, and each thread more idles a
            // millisecond or so before the walk can end, which a small tree
            // pays in full.
            .threads(1)
            .build_parallel()
            .run(|| {
                let mut share = WalkerShare { walked: Walked::default(), sender: sender.clone() };
                Box::new(move |walked| match walked {
                    Ok(entry) => self.visit(entry, &mut share.walked),
                    Err(error) => {
                        share.walked.error.get_or_insert(error);
                        WalkState::Quit
                    }
                })
            });
        drop(sender);
        let mut walked = Walked::default();
        for share in receiver {
            if let Some(error) = share.error {
                return Err(operation_error(format!("cannot walk directory: {error}")));
            }
            walked.files.extend(share.files);
            walked.nested_trees.extend(share.nested_trees);
        }
        Ok(walked)
    }

    fn visit(&self, entry: DirEntry, walked: &mut Walked) -> WalkState {
        let path = entry.path();
        if entry.file_type().is_some_and(|file_type| file_type.is_dir()) {
            if self.filter.excludes_canonical_tree(&self.canonical(path)) {
                return WalkState::Skip;
            }
            // A directory with its own config is another project's tree,
            // walked after this one under that config.
            if self.has_nested_projects
                && entry.depth() > 0
                && path.join(CONFIG_FILE_NAME).is_file()
            {
                let max_depth = self.max_depth.map(|depth| depth.saturating_sub(entry.depth()));
                walked.nested_trees.push((entry.into_path(), max_depth));
                return WalkState::Skip;
            }
            return WalkState::Continue;
        }
        let is_selected =
            path.extension().is_some_and(|extension| extension == "lua" || extension == "luau")
                && self.selection.is_none_or(|(directory, patterns)| {
                    path.strip_prefix(directory).is_ok_and(|relative| patterns.is_match(relative))
                });
        if !is_selected {
            return WalkState::Continue;
        }
        // The walk follows no symlink, so only a symlink itself needs the
        // disk to tell what it is and where it leads.
        let is_included = if entry.path_is_symlink() {
            path.is_file() && self.filter.is_included(path)
        } else {
            #[expect(
                clippy::filetype_is_file,
                reason = "a pipe or device named like a source file is no file to read"
            )]
            let is_file = entry.file_type().is_some_and(|file_type| file_type.is_file());
            is_file && self.filter.includes_canonical(&self.canonical(path))
        };
        if is_included {
            walked.files.push(entry.into_path());
        }
        WalkState::Continue
    }

    /// `path`, which the walk reached, under the tree's canonical form, so the
    /// filter sees a canonical path without asking the disk for one.
    fn canonical(&self, path: &Path) -> PathBuf {
        match path.strip_prefix(self.tree) {
            Ok(below) if !below.as_os_str().is_empty() => self.canonical_tree.join(below),
            Ok(_) | Err(_) => self.canonical_tree.to_path_buf(),
        }
    }
}

/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] when `directory` can't be
/// resolved.
fn canonical_directory(directory: &Path) -> Result<PathBuf, Exit> {
    // Walking up a `..`, which `path::absolute` keeps on Unix, would leave
    // this directory's ancestors.
    FileSystemOs
        .canonicalize(directory)
        .map_err(|e| usage_error(format!("`{}`: {e}", directory.display())))
}

/// Runs `work` over `files` on the rayon pool and hands `take` each outcome
/// on the calling thread, in input order.
///
/// An error from `take` ends the run once the files already started finish.
/// Each file gets an arena from a pool, reset when its outcome is done, so a
/// run allocates about one arena per thread. A file starts as soon as a
/// thread is free and is less than a window of files past the outcome `take`
/// waits for, which keeps memory flat on huge projects without a thread ever
/// waiting for a batch to finish.
///
/// # Errors
///
/// Returns the first error `take` returns.
pub(crate) fn for_each_outcome_in_order<Outcome>(
    files: &[ProjectFile],
    work: impl Fn(&ProjectFile, &Allocator) -> Outcome + Sync,
    mut take: impl FnMut(Outcome) -> Result<(), Exit>,
) -> Result<(), Exit>
where
    Outcome: Send,
{
    let thread_count = rayon::current_num_threads();
    let pool = AllocatorPool::new(thread_count);
    // A smaller window leaves threads idle behind a slow file, and a larger
    // one gains nothing.
    let window = NonZero::new(thread_count.saturating_mul(16)).unwrap_or(NonZero::<usize>::MIN);
    let (sender, receiver) = mpsc::channel::<(usize, thread::Result<Outcome>)>();
    let mut ready: Vec<Option<Outcome>> = iter::repeat_with(|| None).take(window.get()).collect();
    let mut next_outcome = |index: usize| loop {
        if let Some(outcome) = ready.get_mut(index % window)?.take() {
            return Some(outcome);
        }
        // `sender` outlives this, so a job still out always arrives.
        let (finished, result) = receiver.recv().ok()?;
        let outcome = result.unwrap_or_else(|payload| panic::resume_unwind(payload));
        *ready.get_mut(finished % window)? = Some(outcome);
    };
    // Spawned from outside the pool, the jobs queue in file order.
    rayon::in_place_scope(|scope| {
        let start = |index: usize| {
            if let Some(file) = files.get(index) {
                let (job_work, job_pool, job_sender) = (&work, &pool, sender.clone());
                scope.spawn(move |_| {
                    // A panic would otherwise leave the caller waiting forever.
                    let result =
                        panic::catch_unwind(AssertUnwindSafe(|| job_work(file, &job_pool.get())));
                    let _unused = job_sender.send((index, result));
                });
            }
        };
        (0..window.get()).for_each(start);
        for index in 0..files.len() {
            let Some(outcome) = next_outcome(index) else {
                break;
            };
            start(index.saturating_add(window.get()));
            take(outcome)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use luck_token::LuaTarget;

    use super::*;

    fn selected(config_path: Option<&Path>, paths: &[String]) -> Vec<ProjectFile> {
        project_files(config_path, paths, false, None).expect("files resolve")
    }

    fn file_names(files: &[ProjectFile]) -> Vec<String> {
        files
            .iter()
            .map(|file| file.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn target(file: &ProjectFile) -> LuaTarget {
        file.project.target_for(Some(&file.path))
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("a parent directory")).expect("mkdir");
        fs::write(path, contents).expect("write file");
    }

    /// The one-shot commands must honor the project's per-extension dialect,
    /// or a Roblox tree keeping Luau in `.lua` files can't bundle at all.
    #[test]
    fn one_shot_project_is_the_one_above_the_input() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("luck.json"), r#"{"lua":"roblox"}"#)
            .expect("write luck.json");
        let entry = directory.path().join("src/main.lua");
        fs::create_dir_all(entry.parent().expect("a parent")).expect("mkdir src");

        let project = one_shot_project(None, &entry).expect("project loads");
        assert_eq!(project.target_for(Some(&entry)), LuaTarget::LuauRoblox);
    }

    /// With no `luck.json`, the defaults root where discovery started, so a
    /// bundle resolves its modules next to its entry.
    #[test]
    fn one_shot_project_without_a_config_roots_the_defaults_at_the_input() {
        let directory = tempfile::tempdir().expect("tempdir");
        let entry = directory.path().join("sources/main.luau");
        write_file(&entry, "return 1\n");

        let project = one_shot_project(None, &entry).expect("defaults");
        assert_eq!(project.target_for(Some(&entry)), LuaTarget::Luau);
        let canonical_sources =
            entry.parent().and_then(|parent| FileSystemOs.canonicalize(parent).ok());
        if find_config(directory.path()).is_none() {
            assert_eq!(Some(project.root()), canonical_sources.as_deref());
        }
    }

    /// A `.luau` file under a roblox config must resolve to the roblox target,
    /// since the path commands pick each file's parse dialect from it.
    #[test]
    fn files_resolve_each_target_from_the_config() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, r#"{"luau":"roblox"}"#).expect("write luck.json");
        fs::write(directory.path().join("main.lua"), "return 1\n").expect("write main.lua");
        fs::write(directory.path().join("main.luau"), "return 2\n").expect("write main.luau");

        let root = directory.path().display().to_string();
        let targets: Vec<(String, LuaTarget)> = selected(Some(&config_path), &[root])
            .iter()
            .map(|file| (file.path.file_name().unwrap().to_string_lossy().into(), target(file)))
            .collect();
        assert_eq!(
            targets,
            [
                ("main.lua".to_owned(), LuaTarget::Lua54),
                ("main.luau".to_owned(), LuaTarget::LuauRoblox),
            ]
        );
    }

    #[test]
    fn files_take_the_nearest_config_above_them() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(&root.join("luck.json"), r#"{"lua":"lua51"}"#);
        write_file(&root.join("pkg/luck.json"), r#"{"lua":"lua53"}"#);
        write_file(&root.join("a.lua"), "return 1\n");
        write_file(&root.join("pkg/deep/b.lua"), "return 2\n");

        let walked = selected(None, &[root.display().to_string()]);
        let walked_targets: Vec<LuaTarget> = walked.iter().map(target).collect();
        assert_eq!(file_names(&walked), ["a.lua", "b.lua"]);
        assert_eq!(walked_targets, [LuaTarget::Lua51, LuaTarget::Lua53]);

        let named = selected(None, &[root.join("pkg/deep/b.lua").display().to_string()]);
        assert_eq!(target(&named[0]), LuaTarget::Lua53);

        // `-c` puts every file under the one config.
        let pinned = selected(Some(&root.join("luck.json")), &[root.display().to_string()]);
        assert!(pinned.iter().all(|file| target(file) == LuaTarget::Lua51), "{pinned:?}");
        assert_eq!(file_names(&pinned), ["a.lua", "b.lua"]);
    }

    /// A nested project isn't under its parent's globs, but the parent can
    /// still exclude the whole tree it sits in.
    #[test]
    fn nested_project_keeps_its_own_filter() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(&root.join("luck.json"), r#"{"exclude":["**/gen/**","vendor/**"]}"#);
        write_file(&root.join("gen/skip.lua"), "return 1\n");
        write_file(&root.join("pkg/luck.json"), r#"{"exclude":["**/tmp/**"]}"#);
        write_file(&root.join("pkg/gen/keep.lua"), "return 2\n");
        write_file(&root.join("pkg/tmp/skip.lua"), "return 3\n");
        write_file(&root.join("vendor/lib/luck.json"), "{}");
        write_file(&root.join("vendor/lib/skip.lua"), "return 4\n");

        let files = selected(None, &[root.display().to_string()]);
        assert_eq!(file_names(&files), ["keep.lua"]);
    }

    #[test]
    fn named_file_skips_include_but_not_exclude() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(&root.join("luck.json"), r#"{"include":["src/**/*.lua"],"exclude":["gen/**"]}"#);
        write_file(&root.join("tools/run.lua"), "return 1\n");
        write_file(&root.join("gen/out.lua"), "return 2\n");

        let paths = [
            root.join("tools/run.lua").display().to_string(),
            root.join("gen/out.lua").display().to_string(),
        ];
        assert_eq!(file_names(&selected(None, &paths)), ["run.lua"]);
    }

    #[test]
    fn no_matching_file_is_an_error_unless_allowed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let paths = [directory.path().display().to_string()];
        assert_eq!(project_files(None, &paths, false, None).unwrap_err(), Exit::Usage);
        let allowed = project_files(None, &paths, true, None).expect("allowed");
        assert!(allowed.is_empty(), "{allowed:?}");
    }

    #[test]
    fn the_lint_overlay_reaches_every_project() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(&root.join("luck.json"), r#"{"lint":{"extra_globals":["a"]}}"#);
        write_file(&root.join("pkg/luck.json"), r#"{"lint":{"extra_globals":["b"]}}"#);
        write_file(&root.join("a.lua"), "return 1\n");
        write_file(&root.join("pkg/b.lua"), "return 2\n");

        let overlay = LintConfig { extra_globals: vec!["vim".to_owned()], ..LintConfig::default() };
        let files = project_files(None, &[root.display().to_string()], false, Some(&overlay))
            .expect("files resolve");
        let globals: Vec<Vec<String>> = files
            .iter()
            .map(|file| file.project.linter_for(Some(&file.path)).config().extra_globals.clone())
            .collect();
        assert_eq!(globals, [["a", "vim"], ["b", "vim"]]);
    }

    #[test]
    fn collect_tree_honors_exclude_glob() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(
            &root.join("luck.json"),
            r#"{"lua":"lua54","entry":"src/main.lua","exclude":["gen/**"]}"#,
        );
        write_file(&root.join("src/keep.lua"), "return 1\n");
        write_file(&root.join("gen/skip.lua"), "return 2\n");

        // A raw, non-canonical root, as production passes; `ProjectFilter`
        // canonicalizes it.
        let files = selected(None, &[root.display().to_string()]);
        assert_eq!(file_names(&files), ["keep.lua"]);
    }

    #[cfg(unix)]
    #[test]
    fn collect_tree_does_not_enter_an_excluded_tree() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        let excluded = root.join("gen");
        fs::create_dir_all(&excluded).expect("mkdir gen");
        write_file(&root.join("luck.json"), r#"{"exclude":["gen/**"]}"#);
        write_file(&root.join("keep.lua"), "return 1\n");
        fs::set_permissions(&excluded, fs::Permissions::from_mode(0o000)).expect("lock gen");

        let walked = project_files(None, &[root.display().to_string()], false, None);
        fs::set_permissions(&excluded, fs::Permissions::from_mode(0o755)).expect("unlock gen");

        let files = walked.expect("an excluded directory must not fail the walk");
        assert_eq!(file_names(&files), ["keep.lua"]);
    }

    #[test]
    fn collect_tree_default_filter_takes_all_lua() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(&root.join("a.lua"), "return 1\n");
        write_file(&root.join("b.luau"), "return 2\n");
        write_file(&root.join("c.txt"), "nope\n");

        let files = selected(None, &[root.display().to_string()]);
        assert_eq!(file_names(&files), ["a.lua", "b.luau"]);
    }

    #[test]
    fn split_glob_splits_before_the_first_wildcard() {
        let split = |arg: &str| {
            let (base, pattern) = split_glob(arg);
            (base.to_string_lossy().replace('\\', "/"), pattern)
        };
        let pair = |base: &str, pattern: &str| (base.to_owned(), pattern.to_owned());
        assert_eq!(split("src/**/*.lua"), pair("src/", "**/*.lua"));
        assert_eq!(
            split(&format!(".{MAIN_SEPARATOR}src{MAIN_SEPARATOR}*.lua")),
            pair("src/", "*.lua")
        );
        assert_eq!(split("src/gen/a*.lua"), pair("src/gen/", "a*.lua"));
        assert_eq!(split("../pkg/*.lua"), pair("../pkg/", "*.lua"));
        assert_eq!(split("/abs/*.lua"), pair("/abs/", "*.lua"));
        assert_eq!(split("/*.lua"), pair("/", "*.lua"));
        assert_eq!(split("*.lua"), pair(".", "*.lua"));
        assert_eq!(split("{a,b}/x.lua"), pair(".", "{a,b}/x.lua"));
        assert_eq!(split("vendor"), pair("vendor/", ""));
    }

    #[test]
    fn lexical_absolute_resolves_dots_against_the_working_directory() {
        let working_directory = Path::new("/work/project");
        assert_eq!(
            lexical_absolute(working_directory, Path::new("./a/../b/x.lua")),
            Path::new("/work/project/b/x.lua")
        );
        assert_eq!(
            lexical_absolute(working_directory, Path::new("../other")),
            Path::new("/work/other")
        );
    }

    /// `luck lint ../pkg` from a project with its own config must not take
    /// that config: it isn't above `pkg`.
    #[test]
    fn discovery_ignores_a_config_reached_only_through_dot_dot() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(&root.join("here/luck.json"), r#"{"lua":"lua51"}"#);
        write_file(&root.join("pkg/a.lua"), "return 1\n");

        let mut projects = Projects::new(None, &root.join("here"), None).expect("projects");
        let project = projects.for_directory(&root.join("here/../pkg")).expect("project resolves");
        assert_eq!(project.target_for(Some(Path::new("a.lua"))), LuaTarget::Lua54);
    }
}
