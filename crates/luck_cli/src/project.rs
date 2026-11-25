use std::env;
use std::ops::Deref;
use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR, Path, PathBuf};
use std::sync::{Arc, mpsc};

use globset::GlobSet;
use luck_bundler::{BundleOptions, DynamicRequire};
use luck_config::{
    CONFIG_FILE_NAME, Project, ProjectFilter, WalkEntry, WalkError, WalkRoot, WalkVisitor,
    find_config, glob_set, walk_source_files,
};
use luck_linter::LintConfig;
use luck_resolver::{FileSystem as _, FileSystemOs, lexically_normalized};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::exit::{Exit, config_error, operation_error, usage_error};

pub(crate) fn current_directory() -> Result<PathBuf, Exit> {
    env::current_dir().map_err(|e| usage_error(format!("failed to get current directory: {e}")))
}

pub(crate) fn existing_file(input: &str) -> Result<&Path, Exit> {
    let path = Path::new(input);
    if path.is_file() { Ok(path) } else { Err(usage_error(format!("file not found: `{input}`"))) }
}

/// How `bundle` and `graph` resolve and read modules: as `project` says, with
/// `--dynamic-require` replacing its mode when given.
pub(crate) fn bundle_options(
    project: &Project,
    dynamic_require: Option<DynamicRequire>,
) -> BundleOptions<'_> {
    let options = project.bundle_options();
    BundleOptions { dynamic_require: dynamic_require.unwrap_or(options.dynamic_require), ..options }
}

pub(crate) fn load_project(config_path: &Path) -> Result<Project, Exit> {
    Project::load(config_path).map_err(|error| config_error(&error))
}

/// The project a command that takes one input reads it under: `-c` when
/// given, or else the `luck.json` that governs `input`.
///
/// Discovery starts in the input's own directory, or in the input when it's a
/// directory, not cwd, so `luck bundle path/to/project/src/main.luau` sees that
/// project's config. With none, it's the defaults rooted where discovery
/// started, so a bundle resolves its modules from the entry's directory.
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
    match find_config(&start_directory) {
        Some(found) => load_project(&found),
        None => Ok(Project::defaults(start_directory)),
    }
}

/// The project of `-c` when given, or else of the first `luck.json` found
/// walking up from cwd, or else the defaults rooted at cwd.
pub(crate) fn working_project(config_path: Option<&Path>) -> Result<Project, Exit> {
    let working_directory = current_directory()?;
    match config_path.map(Path::to_path_buf).or_else(|| find_config(&working_directory)) {
        Some(found) => load_project(&found),
        None => Ok(Project::defaults(working_directory)),
    }
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
/// loads here, before
/// [`for_each_outcome_in_order`](crate::thread_pool::for_each_outcome_in_order)
/// holds the stdio locks on the calling thread while rayon workers run, so a
/// worker reporting a config error would deadlock.
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
                return Err(usage_error(format!("no `.luau` or `.lua` files matched `{arg}`")));
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
            let absolute_base = lexically_normalized(&working_directory.join(&base));
            dropped.push((absolute_base, glob_set(&patterns).map_err(usage_error)?));
        }
        found.retain(|file| {
            let absolute = lexically_normalized(&working_directory.join(&file.path));
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
        return Err(usage_error(format!("no `.luau` or `.lua` files matched {selection}")));
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
    fn for_directory(&mut self, directory: &Path) -> Result<Arc<Project>, Exit> {
        if let Some(explicit) = &self.explicit {
            return Ok(Arc::clone(explicit));
        }
        self.for_canonical_directory(&canonical_directory(directory)?)
    }

    /// [`for_directory`](Self::for_directory) for a canonical `directory`.
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

    /// Appends the `.luau` and `.lua` files under `directory` that their
    /// project's filter takes and `selection` matches relative to
    /// `directory`, sorted by path.
    fn collect_tree(
        &mut self,
        directory: &Path,
        selection: Option<&Selection>,
        found: &mut Vec<ProjectFile>,
    ) -> Result<(), Exit> {
        let has_nested_projects = self.explicit.is_none();
        let is_selected = |path: &Path| {
            selection.is_none_or(|selected| {
                path.strip_prefix(directory)
                    .is_ok_and(|relative| selected.patterns.is_match(relative))
            })
        };
        let mut tree_files = Vec::new();
        let max_depth = selection.and_then(|selected| selected.max_depth);
        let mut pending_trees = vec![(directory.to_path_buf(), max_depth)];
        while let Some((tree, tree_max_depth)) = pending_trees.pop() {
            let canonical_tree = canonical_directory(&tree)?;
            let project = self.for_canonical_directory(&canonical_tree)?;
            let (nested_tree_sender, nested_trees) = mpsc::channel();
            let visitor = TreeVisitor {
                filter: project.filter(),
                max_depth: tree_max_depth,
                has_nested_projects,
                nested_trees: nested_tree_sender,
            };
            let root = WalkRoot { path: &tree, canonical: &canonical_tree };
            let walked = walk_source_files(&[root], tree_max_depth, &visitor)
                .map_err(|error| operation_error(format!("cannot walk directory: {error}")))?;
            tree_files.extend(
                walked
                    .into_iter()
                    .filter(|file| is_selected(&file.path))
                    .map(|file| ProjectFile { path: file.path, project: Arc::clone(&project) }),
            );
            pending_trees.extend(nested_trees.try_iter());
        }
        tree_files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
        found.extend(tree_files);
        Ok(())
    }
}

/// One project's tree, which a walk reads under that project's filter.
struct TreeVisitor<'tree> {
    filter: &'tree ProjectFilter,
    max_depth: Option<usize>,
    has_nested_projects: bool,
    /// Each directory under the tree with a config of its own, with how much
    /// deeper a glob without `**` can still match in it.
    nested_trees: mpsc::Sender<(PathBuf, Option<usize>)>,
}

impl WalkVisitor for TreeVisitor<'_> {
    type Error = WalkError;

    fn directory_filter(&self, _: &WalkEntry<'_>) -> Option<impl Deref<Target = ProjectFilter>> {
        Some(self.filter)
    }

    fn file_filter(&self, _: &WalkEntry<'_>) -> impl Deref<Target = ProjectFilter> {
        self.filter
    }

    /// A directory with its own config is another project's tree, walked
    /// after this one under that config.
    fn enters(&self, directory: &WalkEntry<'_>) -> bool {
        let is_nested_project = self.has_nested_projects
            && directory.depth > 0
            && directory.path.join(CONFIG_FILE_NAME).is_file();
        if is_nested_project {
            let max_depth = self.max_depth.map(|depth| depth - directory.depth);
            // The receiver outlives the walk.
            let _ = self.nested_trees.send((directory.path.to_path_buf(), max_depth));
        }
        !is_nested_project
    }

    fn read_error(&self, error: WalkError) -> Result<(), WalkError> {
        Err(error)
    }
}

fn canonical_directory(directory: &Path) -> Result<PathBuf, Exit> {
    // Walking up a `..`, which `path::absolute` keeps on Unix, would leave
    // this directory's ancestors.
    FileSystemOs
        .canonicalize(directory)
        .map_err(|e| usage_error(format!("`{}`: {e}", directory.display())))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use luck_stdlib::Variant;

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

    fn variant(file: &ProjectFile) -> Variant {
        file.project.variant()
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("a parent directory")).expect("mkdir");
        fs::write(path, contents).expect("write file");
    }

    #[test]
    fn one_shot_project_is_the_one_above_the_input() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("luck.json"), r#"{"variant":"roblox"}"#)
            .expect("write luck.json");
        let entry = directory.path().join("src/main.luau");
        fs::create_dir_all(entry.parent().expect("a parent")).expect("mkdir src");

        let project = one_shot_project(None, &entry).expect("project loads");
        assert_eq!(project.variant(), Variant::Roblox);
    }

    /// With no `luck.json`, the defaults root where discovery started, so a
    /// bundle resolves its modules next to its entry.
    #[test]
    fn one_shot_project_without_a_config_roots_the_defaults_at_the_input() {
        let directory = tempfile::tempdir().expect("tempdir");
        let entry = directory.path().join("sources/main.luau");
        write_file(&entry, "return 1\n");

        let project = one_shot_project(None, &entry).expect("defaults");
        assert_eq!(project.variant(), Variant::Standalone);
        let canonical_sources =
            entry.parent().and_then(|parent| FileSystemOs.canonicalize(parent).ok());
        if find_config(directory.path()).is_none() {
            assert_eq!(Some(project.root()), canonical_sources.as_deref());
        }
    }

    /// Every `.luau` and `.lua` file under a roblox config gets the roblox
    /// variant, whatever its extension.
    #[test]
    fn files_take_the_variant_of_their_config() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config_path = directory.path().join("luck.json");
        fs::write(&config_path, r#"{"variant":"roblox"}"#).expect("write luck.json");
        fs::write(directory.path().join("main.lua"), "return 1\n").expect("write main.lua");
        fs::write(directory.path().join("main.luau"), "return 2\n").expect("write main.luau");

        let root = directory.path().display().to_string();
        let variants: Vec<(String, Variant)> = selected(Some(&config_path), &[root])
            .iter()
            .map(|file| (file.path.file_name().unwrap().to_string_lossy().into(), variant(file)))
            .collect();
        assert_eq!(
            variants,
            [("main.lua".to_owned(), Variant::Roblox), ("main.luau".to_owned(), Variant::Roblox),]
        );
    }

    #[test]
    fn files_take_the_nearest_config_above_them() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(&root.join("luck.json"), r#"{"variant":"roblox"}"#);
        write_file(&root.join("pkg/luck.json"), r#"{"variant":"standalone"}"#);
        write_file(&root.join("a.luau"), "return 1\n");
        write_file(&root.join("pkg/deep/b.luau"), "return 2\n");

        let walked = selected(None, &[root.display().to_string()]);
        let walked_targets: Vec<Variant> = walked.iter().map(variant).collect();
        assert_eq!(file_names(&walked), ["a.luau", "b.luau"]);
        assert_eq!(walked_targets, [Variant::Roblox, Variant::Standalone]);

        let named = selected(None, &[root.join("pkg/deep/b.luau").display().to_string()]);
        assert_eq!(variant(&named[0]), Variant::Standalone);

        // `-c` puts every file under the one config.
        let pinned = selected(Some(&root.join("luck.json")), &[root.display().to_string()]);
        assert!(pinned.iter().all(|file| variant(file) == Variant::Roblox), "{pinned:?}");
        assert_eq!(file_names(&pinned), ["a.luau", "b.luau"]);
    }

    /// A nested project isn't under its parent's globs, but the parent can
    /// still exclude the whole tree it sits in.
    #[test]
    fn nested_project_keeps_its_own_filter() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(&root.join("luck.json"), r#"{"exclude":["**/gen/**","vendor/**"]}"#);
        write_file(&root.join("gen/skip.luau"), "return 1\n");
        write_file(&root.join("pkg/luck.json"), r#"{"exclude":["**/tmp/**"]}"#);
        write_file(&root.join("pkg/gen/keep.luau"), "return 2\n");
        write_file(&root.join("pkg/tmp/skip.luau"), "return 3\n");
        write_file(&root.join("vendor/lib/luck.json"), "{}");
        write_file(&root.join("vendor/lib/skip.luau"), "return 4\n");

        let files = selected(None, &[root.display().to_string()]);
        assert_eq!(file_names(&files), ["keep.luau"]);
    }

    #[test]
    fn named_file_skips_include_but_not_exclude() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(
            &root.join("luck.json"),
            r#"{"include":["src/**/*.luau"],"exclude":["gen/**"]}"#,
        );
        write_file(&root.join("tools/run.luau"), "return 1\n");
        write_file(&root.join("gen/out.luau"), "return 2\n");

        let paths = [
            root.join("tools/run.luau").display().to_string(),
            root.join("gen/out.luau").display().to_string(),
        ];
        assert_eq!(file_names(&selected(None, &paths)), ["run.luau"]);
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
        write_file(&root.join("a.luau"), "return 1\n");
        write_file(&root.join("pkg/b.luau"), "return 2\n");

        let overlay = LintConfig { extra_globals: vec!["app".to_owned()], ..LintConfig::default() };
        let files = project_files(None, &[root.display().to_string()], false, Some(&overlay))
            .expect("files resolve");
        let globals: Vec<Vec<String>> = files
            .iter()
            .map(|file| file.project.linter_for(Some(&file.path)).config().extra_globals.clone())
            .collect();
        assert_eq!(globals, [["a", "app"], ["b", "app"]]);
    }

    #[test]
    fn collect_tree_honors_exclude_glob() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(&root.join("luck.json"), r#"{"entry":"src/main.luau","exclude":["gen/**"]}"#);
        write_file(&root.join("src/keep.luau"), "return 1\n");
        write_file(&root.join("gen/skip.luau"), "return 2\n");

        // A raw, non-canonical root, as production passes; `ProjectFilter`
        // canonicalizes it.
        let files = selected(None, &[root.display().to_string()]);
        assert_eq!(file_names(&files), ["keep.luau"]);
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
        write_file(&root.join("keep.luau"), "return 1\n");
        fs::set_permissions(&excluded, fs::Permissions::from_mode(0o000)).expect("lock gen");

        let walked = project_files(None, &[root.display().to_string()], false, None);
        fs::set_permissions(&excluded, fs::Permissions::from_mode(0o755)).expect("unlock gen");

        let files = walked.expect("an excluded directory must not fail the walk");
        assert_eq!(file_names(&files), ["keep.luau"]);
    }

    #[test]
    fn collect_tree_default_filter_takes_every_source_file() {
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
        assert_eq!(split("src/**/*.luau"), pair("src/", "**/*.luau"));
        assert_eq!(
            split(&format!(".{MAIN_SEPARATOR}src{MAIN_SEPARATOR}*.luau")),
            pair("src/", "*.luau")
        );
        assert_eq!(split("src/gen/a*.luau"), pair("src/gen/", "a*.luau"));
        assert_eq!(split("../pkg/*.luau"), pair("../pkg/", "*.luau"));
        assert_eq!(split("/abs/*.luau"), pair("/abs/", "*.luau"));
        assert_eq!(split("/*.luau"), pair("/", "*.luau"));
        assert_eq!(split("*.luau"), pair(".", "*.luau"));
        assert_eq!(split("{a,b}/x.luau"), pair(".", "{a,b}/x.luau"));
        assert_eq!(split("vendor"), pair("vendor/", ""));
    }

    /// `luck lint ../pkg` from a project with its own config must not take
    /// that config: it isn't above `pkg`.
    #[test]
    fn discovery_ignores_a_config_reached_only_through_dot_dot() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(&root.join("here/luck.json"), r#"{"variant":"roblox"}"#);
        write_file(&root.join("pkg/a.luau"), "return 1\n");

        let mut projects = Projects::new(None, &root.join("here"), None).expect("projects");
        let project = projects.for_directory(&root.join("here/../pkg")).expect("project resolves");
        assert_eq!(project.variant(), Variant::Standalone);
    }
}
