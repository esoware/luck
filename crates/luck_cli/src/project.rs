//! Project resolution and the walk over a project's files.
//!
//! Picks a target for a one-shot input, loads `luck.json`, expands path
//! arguments into the set of files to process, and runs the path-oriented
//! commands over them in parallel.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use luck_core::{
    DynamicRequire, LuaTarget, LuckConfig, ProjectConfig, ProjectFilter, discover_config,
    load_or_discover_config,
};
use rayon::prelude::*;

use crate::exit::Exit;
use crate::output::{current_directory, usage_error};

/// Resolve the target for the one-shot `bundle`/`minify`/`graph` commands.
///
/// An explicit `-t/--target` wins. Otherwise `config` picks by extension,
/// since a `.lua` file can hold any dialect. With no config, `.luau` is Luau
/// and everything else is Lua 5.4.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] on a bad `-t` value or a
/// bad dialect in `config`.
pub(crate) fn resolve_configured_target(
    target: Option<&str>,
    input_path: &str,
    config: &LuckConfig,
) -> Result<LuaTarget, Exit> {
    target.map_or_else(
        || config.target_for_path(Path::new(input_path)).map_err(usage_error),
        |name| name.parse::<LuaTarget>().map_err(usage_error),
    )
}

/// Resolve how the one-shot commands treat `require(expr)`. An explicit
/// `--dynamic-require` wins over the project's setting, which defaults to
/// warning and keeping the call.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] on a bad
/// `--dynamic-require` value.
pub(crate) fn resolve_dynamic_require(
    config: &LuckConfig,
    flag: Option<&str>,
) -> Result<DynamicRequire, Exit> {
    flag.map_or_else(
        || Ok(config.dynamic_require.unwrap_or_default()),
        |value| value.parse::<DynamicRequire>().map_err(usage_error),
    )
}

/// The config for a one-shot `bundle`, `minify`, or `graph` run.
///
/// That's `-c` resolved through `extends` when given. Otherwise it's the
/// `luck.json` governing the input, or defaults if there's none, and discovery
/// starts in the input's own directory, not cwd, so
/// `luck bundle path/to/project/src/main.lua` sees that project's config. The
/// config directory isn't returned, since a one-shot run names its input and
/// applies no include/exclude filter.
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

/// Expand the user's path arguments into the files of `project` to process,
/// each with its target.
///
/// Empty args default to the current directory. Directories are walked and
/// gated by the include/exclude filter; explicit file args are included
/// unconditionally.
///
/// Every target resolves here, before the parallel commands' locked output
/// sections. Those hold the stdio locks on the main thread while rayon
/// workers run, so a worker reporting a bad dialect would deadlock.
///
/// # Errors
///
/// Reports the failure and returns [`Exit::Usage`] on a bad glob, a path that
/// doesn't exist, or a bad dialect.
pub(crate) fn project_files(
    project: &ProjectConfig,
    paths: &[String],
) -> Result<Vec<(PathBuf, LuaTarget)>, Exit> {
    let filter = ProjectFilter::new(
        &project.root,
        project.config.include.as_deref(),
        project.config.exclude.as_deref(),
    )
    .map_err(usage_error)?;

    let default_paths = [".".to_owned()];
    let requested_paths = if paths.is_empty() { default_paths.as_slice() } else { paths };

    let mut files: Vec<PathBuf> = Vec::new();
    for arg in requested_paths {
        let path = PathBuf::from(arg);
        if path.is_dir() {
            files.extend(collect_lua_files(&path, &filter));
        } else if path.is_file() {
            files.push(path);
        } else {
            return Err(usage_error(format!("path not found: `{arg}`")));
        }
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

fn collect_lua_files(directory: &Path, filter: &ProjectFilter) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(directory).add_custom_ignore_filename(".luckignore").build();
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_file()
            && let Some(extension) = path.extension()
            && (extension == "lua" || extension == "luau")
            && filter.is_included(path)
        {
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use std::fs;

    use luck_core::parse_luck_config;

    use super::*;

    /// Discovers the governing config and resolves against it, as the
    /// one-shot commands do.
    fn target_for(target: Option<&str>, input_path: &str) -> LuaTarget {
        let config = config_for_one_shot(None, input_path).expect("config resolves");
        resolve_configured_target(target, input_path, &config).expect("target resolves")
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
        assert_eq!(target_for(Some("51"), &entry), LuaTarget::Lua51);
    }

    #[test]
    fn configured_target_parses_when_provided() {
        assert_eq!(target_for(Some("54"), "main.luau"), LuaTarget::Lua54);
        assert_eq!(target_for(Some("roblox"), "main.lua"), LuaTarget::LuauRoblox);
    }

    #[test]
    fn resolve_dynamic_require_prefers_the_flag_over_the_config() {
        let config = parse_luck_config(r#"{"dynamic_require":"error"}"#).expect("parse config");
        assert_eq!(resolve_dynamic_require(&config, None), Ok(DynamicRequire::Error));
        assert_eq!(resolve_dynamic_require(&config, Some("allow")), Ok(DynamicRequire::Allow));
        // Warning and keeping the call is the default when nothing says otherwise.
        assert_eq!(resolve_dynamic_require(&LuckConfig::default(), None), Ok(DynamicRequire::Warn));
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
        let files = collect_lua_files(root, &filter);

        let names: Vec<String> = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"keep.lua".to_owned()), "kept file present");
        assert!(!names.contains(&"skip.lua".to_owned()), "excluded file skipped");
    }

    #[test]
    fn collect_lua_files_default_filter_takes_all_lua() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = directory.path();
        fs::write(root.join("a.lua"), "return 1\n").expect("write a");
        fs::write(root.join("b.luau"), "return 2\n").expect("write b");
        fs::write(root.join("c.txt"), "nope\n").expect("write c");

        let filter = ProjectFilter::new(root, None, None).expect("valid globs");
        let files = collect_lua_files(root, &filter);
        let names: Vec<String> = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"a.lua".to_owned()));
        assert!(names.contains(&"b.luau".to_owned()));
        assert!(!names.contains(&"c.txt".to_owned()));
    }
}
