use alloc::sync::Arc;
use core::slice;
use std::env;
use std::path::{self, MAIN_SEPARATOR, Path, PathBuf};
use std::sync::mpsc;

use ignore::WalkBuilder;
use luck_core::{
    CONFIG_FILE_NAME, DynamicRequire, LuaTarget, LuckConfig, ProjectConfig, ProjectFilter,
    discover_config, load_or_discover_config, normalize_path,
};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::exit::{Exit, operation_error, usage_error};

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

/// A `bundle` or `graph` entry, and the root its modules resolve from: the
/// entry's directory, so `luck bundle src/main.lua` finds the modules next to
/// it without `-s`.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] when `entry` isn't a file.
pub(crate) fn entry_and_root(entry: &str) -> Result<(&Path, &Path), Exit> {
    let entry_path = existing_file(entry)?;
    // A bare file name's parent is empty, and an empty root is the working
    // directory.
    Ok((entry_path, entry_path.parent().unwrap_or_else(|| Path::new(""))))
}

/// An explicit `-t/--target` wins. Otherwise `config` picks by extension,
/// since a `.lua` file can hold any dialect. With no config, `.luau` is Luau
/// and everything else is Lua 5.4.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] on a bad dialect in
/// `config`.
pub(crate) fn resolve_configured_target(
    config: &LuckConfig,
    target: Option<LuaTarget>,
    input_path: &str,
) -> Result<LuaTarget, Exit> {
    target.map_or_else(|| config.target_for_path(Path::new(input_path)).map_err(usage_error), Ok)
}

/// An explicit `--dynamic-require` wins over the project's setting, which
/// defaults to warning and keeping the call.
pub(crate) fn resolve_dynamic_require(
    config: &LuckConfig,
    flag: Option<DynamicRequire>,
) -> DynamicRequire {
    flag.or(config.dynamic_require).unwrap_or_default()
}

/// That's `-c` resolved through `extends` when given.
///
/// Otherwise it's the `luck.json` governing the input, or defaults if there's
/// none, and discovery starts in the input's own directory, not cwd, so
/// `luck bundle path/to/project/src/main.lua` sees that project's config.
///
/// The config directory isn't returned, since a one-shot run names its input
/// and applies no include/exclude filter.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] when the working directory
/// is unavailable or the config can't be loaded.
pub(crate) fn config_for_one_shot(
    config_path: Option<&Path>,
    input_path: &str,
) -> Result<LuckConfig, Exit> {
    if config_path.is_some() {
        return Ok(resolve_project(config_path)?.config);
    }

    let start_directory =
        match Path::new(input_path).parent().filter(|parent| !parent.as_os_str().is_empty()) {
            Some(parent) => match parent.canonicalize() {
                Ok(canonical) => canonical,
                Err(_) => current_directory()?.join(parent),
            },
            // A bare file name, or `-` for stdin: cwd is the only context there is.
            None => current_directory()?,
        };

    let discovered = discover_config(&start_directory).map_err(usage_error)?;
    Ok(discovered.map_or_else(LuckConfig::default, |project| project.config))
}

/// `-c` resolved through `extends`, else the first `luck.json` found walking
/// up from cwd, else the defaults rooted at cwd.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] when the working directory
/// is unavailable or the config can't be loaded.
pub(crate) fn resolve_project(config_path: Option<&Path>) -> Result<ProjectConfig, Exit> {
    let working_directory = current_directory()?;
    let loaded = load_or_discover_config(config_path, &working_directory).map_err(usage_error)?;
    Ok(loaded.unwrap_or_else(|| ProjectConfig {
        root: working_directory,
        config: LuckConfig::default(),
    }))
}

/// A `luck.json`, or the defaults when no file is above a path, with what a
/// path command derived from it.
#[derive(Debug)]
pub(crate) struct Project<Settings> {
    pub root: PathBuf,
    pub filter: ProjectFilter,
    pub config: LuckConfig,
    pub settings: Settings,
}

/// A file a path command works on, under the project that governs it.
#[derive(Debug)]
pub(crate) struct ProjectFile<Settings> {
    pub path: PathBuf,
    pub target: LuaTarget,
    pub project: Arc<Project<Settings>>,
}

/// The files `paths` select, each under the nearest `luck.json` above it, or
/// all under `config_path` when `-c` gives one. No path means the working
/// directory.
///
/// A directory is walked and gated by its project's include/exclude filter,
/// and a project nested in it is walked under its own config. A glob selects
/// from the walk of its literal directory part, a file named outright skips
/// `include` but not `exclude`, and a `!` glob drops whatever it matches. A
/// file several arguments reach is processed once.
///
/// `settings_of` runs once per project. Every target resolves here too,
/// before the parallel commands' locked output sections. Those hold the stdio
/// locks on the main thread while rayon workers run, so a worker reporting a
/// bad dialect would deadlock.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] on a config that doesn't
/// load, a bad glob or dialect, a path that doesn't exist, or no file left to
/// work on unless `can_match_nothing`, and [`Exit::Failure`] when a directory
/// can't be walked. An error from `settings_of` is returned as it is.
pub(crate) fn project_files<Settings>(
    config_path: Option<&Path>,
    paths: &[String],
    can_match_nothing: bool,
    settings_of: impl Fn(&LuckConfig) -> Result<Settings, Exit>,
) -> Result<Vec<ProjectFile<Settings>>, Exit> {
    let working_directory = current_directory()?;
    let mut projects = Projects::new(config_path, &working_directory, settings_of)?;

    let (dropped_args, selected_args): (Vec<&str>, Vec<&str>) =
        paths.iter().map(String::as_str).partition(|arg| arg.starts_with('!'));
    let default_args = ["."];
    let requested_args =
        if selected_args.is_empty() { default_args.as_slice() } else { selected_args.as_slice() };

    let mut found: Vec<(PathBuf, Arc<Project<Settings>>)> = Vec::new();
    for arg in requested_args {
        let path = Path::new(arg);
        if path.is_dir() {
            projects.collect_tree(path, None, &mut found)?;
        } else if path.is_file() {
            let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
            let project = projects.for_directory(parent.unwrap_or_else(|| Path::new(".")))?;
            if !project.filter.is_excluded(path) {
                found.push((path.to_path_buf(), project));
            }
        } else if is_glob(arg) {
            let pattern = glob_pattern(arg);
            let selection =
                ProjectFilter::new(&working_directory, Some(slice::from_ref(&pattern)), None)
                    .map_err(usage_error)?;
            let base = glob_base(&pattern);
            if base.is_dir() {
                projects.collect_tree(&base, Some(&selection), &mut found)?;
            }
        } else {
            return Err(usage_error(format!("path not found: `{arg}`")));
        }
    }

    if !dropped_args.is_empty() {
        let patterns: Vec<String> =
            dropped_args.iter().map(|arg| glob_pattern(arg.trim_start_matches('!'))).collect();
        let dropped =
            ProjectFilter::new(&working_directory, None, Some(&patterns)).map_err(usage_error)?;
        found.retain(|(path, _)| !dropped.is_excluded(path));
    }

    // One argument can't reach a file twice, so only several pay for this.
    if requested_args.len() > 1 {
        let mut seen_paths = FxHashSet::default();
        found.retain(|(path, _)| seen_paths.insert(normalize_path(path)));
    }

    if found.is_empty() && !can_match_nothing {
        let quoted_args: Vec<String> = paths.iter().map(|arg| format!("`{arg}`")).collect();
        let selection =
            if quoted_args.is_empty() { "`.`".to_owned() } else { quoted_args.join(", ") };
        return Err(usage_error(format!("no `.lua` or `.luau` files matched {selection}")));
    }

    found
        .into_iter()
        .map(|(path, project)| {
            let target = project.config.target_for_path(&path).map_err(usage_error)?;
            Ok(ProjectFile { path, target, project })
        })
        .collect()
}

/// Whether `arg`, which names no existing path, is a glob and not a typo.
fn is_glob(arg: &str) -> bool {
    arg.contains(['*', '?', '[', '{'])
}

/// A glob argument as `ProjectFilter` matches it: `/`-separated and relative
/// to the working directory.
fn glob_pattern(arg: &str) -> String {
    let slashed = arg.replace(MAIN_SEPARATOR, "/");
    slashed.strip_prefix("./").unwrap_or(&slashed).to_owned()
}

/// The directory part of `pattern` before its first wildcard, which holds
/// everything it can match.
fn glob_base(pattern: &str) -> PathBuf {
    let literal_segments: Vec<&str> =
        pattern.split('/').take_while(|segment| !is_glob(segment)).collect();
    if literal_segments.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(literal_segments.join("/"))
    }
}

/// The projects a run has met, one per `luck.json`.
struct Projects<Settings, SettingsOf> {
    /// Set by `-c`, which puts every file under that one config.
    explicit: Option<Arc<Project<Settings>>>,
    /// The defaults, rooted at the working directory, for a path with no
    /// `luck.json` above it.
    fallback: Option<Arc<Project<Settings>>>,
    working_directory: PathBuf,
    discovered: Vec<Arc<Project<Settings>>>,
    by_directory: FxHashMap<PathBuf, Arc<Project<Settings>>>,
    settings_of: SettingsOf,
}

impl<Settings, SettingsOf> Projects<Settings, SettingsOf>
where
    SettingsOf: Fn(&LuckConfig) -> Result<Settings, Exit>,
{
    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Usage`] when `config_path`
    /// doesn't load, and the error `settings_of` returns for it.
    fn new(
        config_path: Option<&Path>,
        working_directory: &Path,
        settings_of: SettingsOf,
    ) -> Result<Self, Exit> {
        let mut projects = Self {
            explicit: None,
            fallback: None,
            working_directory: working_directory.to_path_buf(),
            discovered: Vec::new(),
            by_directory: FxHashMap::default(),
            settings_of,
        };
        if config_path.is_some() {
            projects.explicit = Some(projects.project(resolve_project(config_path)?)?);
        }
        Ok(projects)
    }

    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Usage`] on a bad glob in
    /// `loaded`, and the error `settings_of` returns for it.
    fn project(&self, loaded: ProjectConfig) -> Result<Arc<Project<Settings>>, Exit> {
        let ProjectConfig { root, config } = loaded;
        let filter = config.project_filter(&root).map_err(usage_error)?;
        let settings = (self.settings_of)(&config)?;
        Ok(Arc::new(Project { root, filter, config, settings }))
    }

    /// The project of the nearest `luck.json` at or above `directory`.
    ///
    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Usage`] when that config
    /// doesn't load, and the error `settings_of` returns for it.
    fn for_directory(&mut self, directory: &Path) -> Result<Arc<Project<Settings>>, Exit> {
        if let Some(explicit) = &self.explicit {
            return Ok(Arc::clone(explicit));
        }
        let absolute = path::absolute(directory)
            .map_err(|e| usage_error(format!("`{}`: {e}", directory.display())))?;
        if let Some(known) = self.by_directory.get(&absolute) {
            return Ok(Arc::clone(known));
        }
        let project = if let Some(loaded) = discover_config(&absolute).map_err(usage_error)? {
            if let Some(known) = self.discovered.iter().find(|known| known.root == loaded.root) {
                Arc::clone(known)
            } else {
                let discovered = self.project(loaded)?;
                self.discovered.push(Arc::clone(&discovered));
                discovered
            }
        } else if let Some(fallback) = &self.fallback {
            Arc::clone(fallback)
        } else {
            let fallback = self.project(ProjectConfig {
                root: self.working_directory.clone(),
                config: LuckConfig::default(),
            })?;
            self.fallback = Some(Arc::clone(&fallback));
            fallback
        };
        self.by_directory.insert(absolute, Arc::clone(&project));
        Ok(project)
    }

    /// Appends the Lua files under `directory` that their project's filter
    /// takes and `selection` matches, sorted by path.
    ///
    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Failure`] when part of
    /// `directory` outside any excluded tree can't be walked, since the files
    /// in it would go unprocessed.
    fn collect_tree(
        &mut self,
        directory: &Path,
        selection: Option<&ProjectFilter>,
        found: &mut Vec<(PathBuf, Arc<Project<Settings>>)>,
    ) -> Result<(), Exit> {
        let has_nested_projects = self.explicit.is_none();
        let mut tree_files = Vec::new();
        let mut pending_trees = vec![directory.to_path_buf()];
        while let Some(tree) = pending_trees.pop() {
            let project = self.for_directory(&tree)?;
            let tree_filter = project.filter.clone();
            let (nested_sender, nested_receiver) = mpsc::channel();
            let walker = WalkBuilder::new(&tree)
                .add_custom_ignore_filename(".luckignore")
                .filter_entry(move |entry| {
                    if !entry.file_type().is_some_and(|file_type| file_type.is_dir()) {
                        return true;
                    }
                    if tree_filter.excludes_tree(entry.path()) {
                        return false;
                    }
                    // A directory with its own config is another project's
                    // tree, walked after this one under that config.
                    if has_nested_projects
                        && entry.depth() > 0
                        && entry.path().join(CONFIG_FILE_NAME).is_file()
                    {
                        let _unused = nested_sender.send(entry.path().to_path_buf());
                        return false;
                    }
                    true
                })
                .build();
            for walked in walker {
                let entry =
                    walked.map_err(|e| operation_error(format!("cannot walk directory: {e}")))?;
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "lua" || extension == "luau")
                    && path.is_file()
                    && project.filter.is_included(path)
                    && selection.is_none_or(|selected| selected.is_included(path))
                {
                    tree_files.push((entry.into_path(), Arc::clone(&project)));
                }
            }
            pending_trees.extend(nested_receiver.try_iter());
        }
        tree_files.sort_by(|(left, _), (right, _)| left.cmp(right));
        found.extend(tree_files);
        Ok(())
    }
}

/// Runs `work` over `files` on the rayon pool and yields the outcomes in input
/// order, so output stays deterministic.
///
/// Files go out in fixed-size chunks, and a chunk only starts once the caller
/// has taken every outcome of the one before, which keeps memory flat on huge
/// projects.
pub(crate) fn outcomes_in_order<Settings, Outcome>(
    files: &[ProjectFile<Settings>],
    work: impl Fn(&ProjectFile<Settings>) -> Outcome + Sync,
) -> impl Iterator<Item = Outcome>
where
    Settings: Send + Sync,
    Outcome: Send,
{
    // Four per thread keeps the pool busy while bounding how many rendered
    // outcomes sit in memory at once.
    let files_per_chunk = rayon::current_num_threads().saturating_mul(4);
    files
        .chunks(files_per_chunk)
        .flat_map(move |chunk| chunk.par_iter().map(&work).collect::<Vec<Outcome>>())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use luck_core::parse_luck_config;

    use super::*;

    fn selected(config_path: Option<&Path>, paths: &[String]) -> Vec<ProjectFile<()>> {
        project_files(config_path, paths, false, |_| Ok(())).expect("files resolve")
    }

    fn file_names(files: &[ProjectFile<()>]) -> Vec<String> {
        files
            .iter()
            .map(|file| file.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("a parent directory")).expect("mkdir");
        fs::write(path, contents).expect("write file");
    }

    fn target_for(target: Option<LuaTarget>, input_path: &str) -> LuaTarget {
        let config = config_for_one_shot(None, input_path).expect("config resolves");
        resolve_configured_target(&config, target, input_path).expect("target resolves")
    }

    /// Discovery finds no config in a fresh tempdir, so this tests the
    /// defaults.
    #[test]
    fn configured_target_infers_from_extension() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = |name: &str| directory.path().join(name).display().to_string();

        assert_eq!(target_for(None, &path("main.luau")), LuaTarget::Luau);
        assert_eq!(target_for(None, &path("main.lua")), LuaTarget::Lua54);
        // No extension falls back to Lua54.
        assert_eq!(target_for(None, &path("main")), LuaTarget::Lua54);
    }

    /// The one-shot commands must honor the project's per-extension dialect,
    /// or a Roblox tree keeping Luau in `.lua` files can't bundle at all.
    #[test]
    fn configured_target_honors_project_config() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::write(directory.path().join("luck.json"), r#"{"lua":"roblox"}"#)
            .expect("write luck.json");
        let source_directory = directory.path().join("src");
        fs::create_dir_all(&source_directory).expect("mkdir src");

        let entry = source_directory.join("main.lua").display().to_string();
        assert_eq!(target_for(None, &entry), LuaTarget::LuauRoblox);
        // An explicit -t still wins over the config.
        assert_eq!(target_for(Some(LuaTarget::Lua51), &entry), LuaTarget::Lua51);
    }

    #[test]
    fn configured_target_prefers_an_explicit_target_over_the_extension() {
        let config = LuckConfig::default();
        assert_eq!(
            resolve_configured_target(&config, Some(LuaTarget::Lua54), "main.luau"),
            Ok(LuaTarget::Lua54)
        );
        assert_eq!(
            resolve_configured_target(&config, Some(LuaTarget::LuauRoblox), "main.lua"),
            Ok(LuaTarget::LuauRoblox)
        );
    }

    #[test]
    fn resolve_dynamic_require_prefers_the_flag_over_the_config() {
        let config = parse_luck_config(r#"{"dynamic_require":"error"}"#).expect("parse config");
        assert_eq!(resolve_dynamic_require(&config, None), DynamicRequire::Error);
        assert_eq!(
            resolve_dynamic_require(&config, Some(DynamicRequire::Allow)),
            DynamicRequire::Allow
        );
        // Warning and keeping the call is the default when nothing says otherwise.
        assert_eq!(resolve_dynamic_require(&LuckConfig::default(), None), DynamicRequire::Warn);
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
            .into_iter()
            .map(|file| (file.path.file_name().unwrap().to_string_lossy().into(), file.target))
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
        let walked_targets: Vec<LuaTarget> = walked.iter().map(|file| file.target).collect();
        assert_eq!(file_names(&walked), ["a.lua", "b.lua"]);
        assert_eq!(walked_targets, [LuaTarget::Lua51, LuaTarget::Lua53]);

        let named = selected(None, &[root.join("pkg/deep/b.lua").display().to_string()]);
        assert_eq!(named[0].target, LuaTarget::Lua53);

        // `-c` puts every file under the one config.
        let pinned = selected(Some(&root.join("luck.json")), &[root.display().to_string()]);
        assert!(pinned.iter().all(|file| file.target == LuaTarget::Lua51), "{pinned:?}");
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
        assert_eq!(project_files(None, &paths, false, |_| Ok(())).unwrap_err(), Exit::Usage);
        let allowed = project_files(None, &paths, true, |_| Ok(())).expect("allowed");
        assert!(allowed.is_empty(), "{allowed:?}");
    }

    #[test]
    fn settings_come_from_each_file_s_own_project() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        write_file(&root.join("luck.json"), r#"{"lua":"lua51"}"#);
        write_file(&root.join("pkg/luck.json"), r#"{"lua":"lua53"}"#);
        write_file(&root.join("a.lua"), "return 1\n");
        write_file(&root.join("pkg/b.lua"), "return 2\n");

        let files = project_files(None, &[root.display().to_string()], false, |config| {
            Ok(config.lua.clone())
        })
        .expect("files resolve");
        let settings: Vec<Option<&str>> =
            files.iter().map(|file| file.project.settings.as_deref()).collect();
        assert_eq!(settings, [Some("lua51"), Some("lua53")]);
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

        let walked = project_files(None, &[root.display().to_string()], false, |_| Ok(()));
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
    fn glob_pattern_is_slashed_and_relative() {
        assert_eq!(glob_pattern("./src/*.lua"), "src/*.lua");
        assert_eq!(glob_pattern(&format!("src{MAIN_SEPARATOR}*.lua")), "src/*.lua");
    }

    #[test]
    fn glob_base_is_the_part_before_the_first_wildcard() {
        assert_eq!(glob_base("src/**/*.lua"), PathBuf::from("src"));
        assert_eq!(glob_base("src/gen/a*.lua"), PathBuf::from("src/gen"));
        assert_eq!(glob_base("*.lua"), PathBuf::from("."));
        assert_eq!(glob_base("{a,b}/x.lua"), PathBuf::from("."));
    }
}
