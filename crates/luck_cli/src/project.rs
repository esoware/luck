use std::env;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use luck_core::{
    DynamicRequire, LuaTarget, LuckConfig, ProjectConfig, ProjectFilter, discover_config,
    load_or_discover_config, normalize_path,
};
use rayon::prelude::*;
use rustc_hash::FxHashSet;

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

/// The project of a path-oriented command: `-c` resolved through `extends`,
/// else the first `luck.json` found walking up from cwd, else the defaults
/// rooted at cwd.
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

/// Empty args default to the current directory.
///
/// Directories are walked and gated by the include/exclude filter; explicit
/// file args are included unconditionally. A file several arguments reach is
/// processed once.
///
/// Every target resolves here, before the parallel commands' locked output
/// sections. Those hold the stdio locks on the main thread while rayon
/// workers run, so a worker reporting a bad dialect would deadlock.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] on a bad glob, a path that
/// doesn't exist, or a bad dialect, and [`Exit::Failure`] when a directory
/// can't be walked.
pub(crate) fn project_files(
    project: &ProjectConfig,
    paths: &[String],
) -> Result<Vec<(PathBuf, LuaTarget)>, Exit> {
    let filter = project.config.project_filter(&project.root).map_err(usage_error)?;

    let default_paths = [".".to_owned()];
    let requested_paths = if paths.is_empty() { default_paths.as_slice() } else { paths };

    let mut files: Vec<PathBuf> = Vec::new();
    for arg in requested_paths {
        let path = PathBuf::from(arg);
        if path.is_dir() {
            files.extend(collect_lua_files(&path, &filter)?);
        } else if path.is_file() {
            files.push(path);
        } else {
            return Err(usage_error(format!("path not found: `{arg}`")));
        }
    }

    // One argument can't reach a file twice, so only several pay for this.
    if requested_paths.len() > 1 {
        let mut seen_paths = FxHashSet::default();
        files.retain(|path| seen_paths.insert(normalize_path(path)));
    }

    files
        .into_iter()
        .map(|path| {
            let target = project.config.target_for_path(&path).map_err(usage_error)?;
            Ok((path, target))
        })
        .collect()
}

/// Runs `work` over `files` on the rayon pool and yields the outcomes in input
/// order, so output stays deterministic.
///
/// Files go out in fixed-size chunks, and a chunk only starts once the caller
/// has taken every outcome of the one before, which keeps memory flat on huge
/// projects.
pub(crate) fn outcomes_in_order<Outcome>(
    files: &[(PathBuf, LuaTarget)],
    work: impl Fn(&Path, LuaTarget) -> Outcome + Sync,
) -> impl Iterator<Item = Outcome>
where
    Outcome: Send,
{
    // Four per thread keeps the pool busy while bounding how many rendered
    // outcomes sit in memory at once.
    let files_per_chunk = rayon::current_num_threads().saturating_mul(4);
    files.chunks(files_per_chunk).flat_map(move |chunk| {
        chunk.par_iter().map(|(path, target)| work(path, *target)).collect::<Vec<Outcome>>()
    })
}

/// # Errors
///
/// Reports the failure and returns [`Exit::Failure`] when part of `directory`
/// outside any excluded tree can't be walked, since the files in it would go
/// unprocessed.
fn collect_lua_files(directory: &Path, filter: &ProjectFilter) -> Result<Vec<PathBuf>, Exit> {
    let tree_filter = filter.clone();
    let walker = WalkBuilder::new(directory)
        .add_custom_ignore_filename(".luckignore")
        .filter_entry(move |entry| {
            !(entry.file_type().is_some_and(|file_type| file_type.is_dir())
                && tree_filter.excludes_tree(entry.path()))
        })
        .build();
    let mut files: Vec<PathBuf> = Vec::new();
    for walked in walker {
        let entry = walked.map_err(|e| operation_error(format!("cannot walk directory: {e}")))?;
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "lua" || extension == "luau")
            && path.is_file()
            && filter.is_included(path)
        {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use luck_core::parse_luck_config;

    use super::*;

    fn file_names(files: &[PathBuf]) -> Vec<String> {
        files.iter().map(|path| path.file_name().unwrap().to_string_lossy().into_owned()).collect()
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

        let project = resolve_project(Some(&config_path)).expect("config resolves");
        let root = directory.path().display().to_string();
        let targets: Vec<(String, LuaTarget)> = project_files(&project, &[root])
            .expect("files resolve")
            .into_iter()
            .map(|(path, target)| (path.file_name().unwrap().to_string_lossy().into(), target))
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
    fn collect_lua_files_honors_exclude_glob() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        fs::write(
            root.join("luck.json"),
            r#"{"lua":"lua54","entry":"src/main.lua","exclude":["gen/**"]}"#,
        )
        .expect("write luck.json");

        let source_directory = root.join("src");
        let generated = root.join("gen");
        fs::create_dir_all(&source_directory).expect("mkdir src");
        fs::create_dir_all(&generated).expect("mkdir gen");
        fs::write(source_directory.join("keep.lua"), "return 1\n").expect("write keep");
        fs::write(generated.join("skip.lua"), "return 2\n").expect("write skip");

        // A raw, non-canonical root, as production passes; `ProjectFilter`
        // canonicalizes it.
        let filter =
            ProjectFilter::new(root, None, Some(&["gen/**".to_owned()])).expect("valid globs");
        let files = collect_lua_files(root, &filter).expect("directory walks");

        let names = file_names(&files);
        assert!(names.contains(&"keep.lua".to_owned()), "kept file present");
        assert!(!names.contains(&"skip.lua".to_owned()), "excluded file skipped");
    }

    #[cfg(unix)]
    #[test]
    fn collect_lua_files_does_not_enter_an_excluded_tree() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        let excluded = root.join("gen");
        fs::create_dir_all(&excluded).expect("mkdir gen");
        fs::write(root.join("keep.lua"), "return 1\n").expect("write keep");
        fs::set_permissions(&excluded, fs::Permissions::from_mode(0o000)).expect("lock gen");

        let filter =
            ProjectFilter::new(root, None, Some(&["gen/**".to_owned()])).expect("valid globs");
        let walked = collect_lua_files(root, &filter);
        fs::set_permissions(&excluded, fs::Permissions::from_mode(0o755)).expect("unlock gen");

        let files = walked.expect("an excluded directory must not fail the walk");
        assert_eq!(file_names(&files), ["keep.lua"]);
    }

    #[test]
    fn collect_lua_files_default_filter_takes_all_lua() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        fs::write(root.join("a.lua"), "return 1\n").expect("write a");
        fs::write(root.join("b.luau"), "return 2\n").expect("write b");
        fs::write(root.join("c.txt"), "nope\n").expect("write c");

        let filter = ProjectFilter::new(root, None, None).expect("valid globs");
        let files = collect_lua_files(root, &filter).expect("directory walks");
        let names = file_names(&files);
        assert!(names.contains(&"a.lua".to_owned()));
        assert!(names.contains(&"b.luau".to_owned()));
        assert!(!names.contains(&"c.txt".to_owned()));
    }
}
