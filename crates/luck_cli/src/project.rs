//! Project resolution for the path-oriented commands: picking a target for a
//! one-shot input, loading `luck.json`, and expanding path arguments into the
//! set of files to process.

use crate::EXIT_USAGE;
use crate::output::current_dir_or_exit;
use luck_core::config::{LuckConfig, ProjectFilter};
use luck_core::types::{DynamicRequire, LuaTarget};
use std::path::{Path, PathBuf};
use std::process;

/// Resolve the target for the one-shot `bundle`/`minify`/`graph` commands.
///
/// An explicit `-t/--target` is parsed via the alias-rich `FromStr`; a bad
/// value exits with code 2. When omitted, `config` decides per extension,
/// because an extension alone cannot say which dialect a `.lua` file is
/// written in. With no config in scope the defaults reproduce plain
/// inference: `.luau` is Luau, everything else Lua 5.4.
pub(crate) fn resolve_configured_target(
    target: Option<&str>,
    input_path: &str,
    config: &LuckConfig,
) -> LuaTarget {
    if let Some(target_str) = target {
        return target_str.parse::<LuaTarget>().unwrap_or_else(|error| {
            eprintln!("Error: {error}");
            process::exit(EXIT_USAGE as i32);
        });
    }

    config
        .target_for_path(Path::new(input_path))
        .unwrap_or_else(|message| {
            eprintln!("Error: {message}");
            process::exit(EXIT_USAGE as i32);
        })
}

/// Resolve how the one-shot commands treat `require(expr)`. An explicit
/// `--dynamic-require` wins over the project's setting, which defaults to
/// warning and keeping the call.
pub(crate) fn resolve_dynamic_require(config: &LuckConfig, flag: Option<&str>) -> DynamicRequire {
    match flag {
        Some(value) => value.parse().unwrap_or_else(|error| {
            eprintln!("Error: {error}");
            process::exit(EXIT_USAGE as i32);
        }),
        None => config.dynamic_require.unwrap_or_default(),
    }
}

/// The `luck.json` governing a one-shot input, discovered upward from the
/// input file's own directory rather than from cwd, so `luck bundle
/// path/to/project/src/main.lua` still sees that project's config.
pub(crate) fn config_governing(input_path: &str) -> LuckConfig {
    let start_dir = match Path::new(input_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent
            .canonicalize()
            .unwrap_or_else(|_| current_dir_or_exit().join(parent)),
        // A bare file name, or `-` for stdin: cwd is the only context there is.
        None => current_dir_or_exit(),
    };

    match luck_core::config::discover_config(&start_dir) {
        Ok(Some((_, config))) => config,
        Ok(None) => LuckConfig::default(),
        Err(message) => {
            eprintln!("Error: {message}");
            process::exit(EXIT_USAGE as i32);
        }
    }
}

/// The config a one-shot `bundle`/`minify` run answers to: an explicit `-c`
/// resolved through `extends`, otherwise the one governing the input file.
/// The config directory is not returned, because a one-shot run names its
/// input directly, so no include/exclude filter is rooted anywhere.
pub(crate) fn config_for_one_shot(config: Option<&Path>, input_path: &str) -> LuckConfig {
    match config {
        Some(path) => resolve_project_config(Some(path)).0,
        None => config_governing(input_path),
    }
}

/// Resolve the project config for the path-oriented subcommands: an explicit
/// `-c` (via extends) or upward discovery from cwd, else defaults. Exits with
/// `EXIT_USAGE` on error. Returns the config and the directory that roots
/// include/exclude globs.
pub(crate) fn resolve_project_config(config: Option<&Path>) -> (LuckConfig, PathBuf) {
    use luck_core::config::{discover_config, load_with_extends};

    let cwd = current_dir_or_exit();

    if let Some(path) = config {
        match load_with_extends(path) {
            Ok(config) => {
                let dir = path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| cwd.clone());
                (config, dir)
            }
            Err(message) => {
                eprintln!("Error: {message}");
                process::exit(EXIT_USAGE as i32);
            }
        }
    } else {
        match discover_config(&cwd) {
            Ok(Some((path, config))) => {
                let dir = path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| cwd.clone());
                (config, dir)
            }
            Ok(None) => (LuckConfig::default(), cwd.clone()),
            Err(message) => {
                eprintln!("Error: {message}");
                process::exit(EXIT_USAGE as i32);
            }
        }
    }
}

/// Build the include/exclude filter rooted at the config directory, exiting
/// `EXIT_USAGE` on a bad glob.
pub(crate) fn project_filter(config_dir: &Path, config: &LuckConfig) -> ProjectFilter {
    ProjectFilter::new(config_dir, &config.include, &config.exclude).unwrap_or_else(|error| {
        eprintln!("Error: {error}");
        process::exit(EXIT_USAGE as i32);
    })
}

/// Expand the user's path arguments into the set of files to process.
/// Empty args default to the current directory. Directories are walked and
/// gated by the project filter; explicit file args are included
/// unconditionally. Exits `EXIT_USAGE` if a path does not exist.
pub(crate) fn collect_target_files(paths: &[String], filter: &ProjectFilter) -> Vec<PathBuf> {
    let paths: Vec<String> = if paths.is_empty() {
        vec![".".to_string()]
    } else {
        paths.to_vec()
    };

    let mut files: Vec<PathBuf> = Vec::new();
    for raw in &paths {
        let path = PathBuf::from(raw);
        if path.is_dir() {
            files.extend(collect_lua_files(&path, filter));
        } else if path.is_file() {
            files.push(path);
        } else {
            eprintln!("Error: path not found: {raw}");
            process::exit(EXIT_USAGE as i32);
        }
    }
    files
}

/// Resolve every file's target up front, exiting `EXIT_USAGE` on a bad
/// dialect. The parallel commands must call this before their locked
/// output sections: those hold the stdio locks on the main thread while
/// rayon workers run, so an `eprintln!` + `process::exit` from a worker
/// deadlocks against the lock instead of exiting.
pub(crate) fn resolve_file_targets(
    files: Vec<PathBuf>,
    config: &LuckConfig,
) -> Vec<(PathBuf, LuaTarget)> {
    files
        .into_iter()
        .map(|path| {
            let target = config.target_for_path(&path).unwrap_or_else(|message| {
                eprintln!("Error: {message}");
                process::exit(EXIT_USAGE as i32);
            });
            (path, target)
        })
        .collect()
}

pub(crate) fn collect_lua_files(dir: &Path, filter: &ProjectFilter) -> Vec<PathBuf> {
    use ignore::WalkBuilder;

    let mut files = Vec::new();
    let walker = WalkBuilder::new(dir)
        .add_custom_ignore_filename(".luckignore")
        .build();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_file()
            && let Some(ext) = path.extension()
            && (ext == "lua" || ext == "luau")
        {
            // include/exclude globs from luck.json gate the walk; the
            // filter compares against canonical paths so strip_prefix
            // against the (canonical) rc_dir lines up.
            let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            if filter.is_included(&abs) {
                files.push(path.to_path_buf());
            }
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The production composition the one-shot commands use: discover the
    /// governing config, then resolve against it.
    fn target_for(target: Option<&str>, input_path: &str) -> LuaTarget {
        resolve_configured_target(target, input_path, &config_governing(input_path))
    }

    /// Paths inside a config-free tempdir, so discovery finds nothing and the
    /// defaults are what is under test.
    #[test]
    fn configured_target_infers_from_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = |name: &str| dir.path().join(name).display().to_string();

        assert_eq!(target_for(None, &path("main.luau")), LuaTarget::Luau);
        assert_eq!(target_for(None, &path("main.lua")), LuaTarget::Lua54);
        // No extension falls back to Lua54.
        assert_eq!(target_for(None, &path("main")), LuaTarget::Lua54);
    }

    /// The one-shot commands must honour the project's per-extension dialect,
    /// or a Roblox tree keeping Luau in `.lua` files cannot bundle at all.
    #[test]
    fn configured_target_honors_project_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("luck.json"), r#"{"lua":"roblox"}"#)
            .expect("write luck.json");
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).expect("mkdir src");

        let entry = src.join("main.lua").display().to_string();
        assert_eq!(target_for(None, &entry), LuaTarget::LuauRoblox);
        // An explicit -t still wins over the config.
        assert_eq!(target_for(Some("51"), &entry), LuaTarget::Lua51);
    }

    #[test]
    fn configured_target_parses_when_provided() {
        assert_eq!(target_for(Some("54"), "main.luau"), LuaTarget::Lua54);
        assert_eq!(
            target_for(Some("roblox"), "main.lua"),
            LuaTarget::LuauRoblox
        );
    }

    #[test]
    fn resolve_dynamic_require_prefers_the_flag_over_the_config() {
        let config = luck_core::config::parse_luck_config(r#"{"dynamic_require":"error"}"#)
            .expect("parse config");
        assert_eq!(
            resolve_dynamic_require(&config, None),
            DynamicRequire::Error
        );
        assert_eq!(
            resolve_dynamic_require(&config, Some("allow")),
            DynamicRequire::Allow
        );
        // Warning and keeping the call is the default when nothing says otherwise.
        assert_eq!(
            resolve_dynamic_require(&LuckConfig::default(), None),
            DynamicRequire::Warn
        );
    }

    #[test]
    fn collect_lua_files_honors_exclude_glob() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(
            root.join("luck.json"),
            r#"{"lua":"lua54","entry":"src/main.lua","exclude":["gen/**"]}"#,
        )
        .expect("write luck.json");

        let src = root.join("src");
        let generated = root.join("gen");
        std::fs::create_dir_all(&src).expect("mkdir src");
        std::fs::create_dir_all(&generated).expect("mkdir gen");
        std::fs::write(src.join("keep.lua"), "return 1\n").expect("write keep");
        std::fs::write(generated.join("skip.lua"), "return 2\n").expect("write skip");

        // Passing the RAW (non-canonical) root proves the production path:
        // ProjectFilter canonicalizes internally.
        let filter = ProjectFilter::new(root, &None, &Some(vec!["gen/**".to_string()]))
            .expect("valid globs");
        let files = collect_lua_files(root, &filter);

        let names: Vec<String> = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"keep.lua".to_string()), "kept file present");
        assert!(
            !names.contains(&"skip.lua".to_string()),
            "excluded file skipped"
        );
    }

    #[test]
    fn collect_lua_files_default_filter_takes_all_lua() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("a.lua"), "return 1\n").expect("write a");
        std::fs::write(root.join("b.luau"), "return 2\n").expect("write b");
        std::fs::write(root.join("c.txt"), "nope\n").expect("write c");

        let filter = ProjectFilter::new(root, &None, &None).expect("valid globs");
        let files = collect_lua_files(root, &filter);
        let names: Vec<String> = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"a.lua".to_string()));
        assert!(names.contains(&"b.luau".to_string()));
        assert!(!names.contains(&"c.txt".to_string()));
    }
}
