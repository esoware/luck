//! Project resolution for the path-oriented commands: picking a target for a
//! one-shot input, loading `luck.json`, and expanding path arguments into the
//! set of files to process.

use crate::EXIT_USAGE;
use crate::output::current_dir_or_exit;
use luck_core::config::{LuckConfig, ProjectFilter};
use luck_core::types::LuaTarget;
use std::path::{Path, PathBuf};
use std::process;

/// Resolve the target for the one-shot `bundle`/`minify`/`graph` commands.
///
/// An explicit `-t/--target` is parsed via the alias-rich `FromStr`; a bad
/// value exits with code 2. When omitted, the target is inferred from the
/// primary input file's extension: `.luau` maps to Luau, everything else to
/// Lua 5.4.
pub(crate) fn resolve_explicit_target(target: Option<&str>, input_path: &str) -> LuaTarget {
    if let Some(target_str) = target {
        return target_str.parse::<LuaTarget>().unwrap_or_else(|error| {
            eprintln!("Error: {error}");
            process::exit(EXIT_USAGE as i32);
        });
    }

    if Path::new(input_path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("luau"))
    {
        LuaTarget::Luau
    } else {
        LuaTarget::Lua54
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

    #[test]
    fn resolve_explicit_target_infers_from_extension() {
        assert_eq!(resolve_explicit_target(None, "main.luau"), LuaTarget::Luau);
        assert_eq!(resolve_explicit_target(None, "main.lua"), LuaTarget::Lua54);
        // No extension falls back to Lua54.
        assert_eq!(resolve_explicit_target(None, "main"), LuaTarget::Lua54);
    }

    #[test]
    fn resolve_explicit_target_parses_when_provided() {
        assert_eq!(
            resolve_explicit_target(Some("54"), "main.luau"),
            LuaTarget::Lua54
        );
        assert_eq!(
            resolve_explicit_target(Some("roblox"), "main.lua"),
            LuaTarget::LuauRoblox
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
