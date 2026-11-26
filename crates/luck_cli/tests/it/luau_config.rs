//! A `.config.luau`, Luau's config written in Luau, which only Luau's own
//! Config library runs, read wherever a `.luaurc` is.

use std::fs;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use crate::common::{run, stderr_text, write_file};

const ALIASES: &str = "\
local lib = \"./lib\"
return { luau = { languagemode = \"strict\", aliases = { lib = lib } } }
";

/// A project whose `main.luau` requires `@lib/name`, which its config
/// names, and builds `main.luau` into `out.luau`.
fn project(config_name: &str, config: &str) -> TempDir {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", r#"{ "entry": "main.luau", "output": "out.luau" }"#);
    write_file(directory.path(), config_name, config);
    write_file(directory.path(), "lib/name.luau", "return \"luck\"\n");
    let main = "\
local name = require(\"@lib/name\")
local count: number = name
return count
";
    write_file(directory.path(), "main.luau", main);
    directory
}

#[test]
fn build_resolves_the_aliases_of_a_config_luau() {
    let directory = project(".config.luau", ALIASES);

    let output = run(directory.path(), &["build", "--quiet"]);

    assert!(output.status.success(), "{}", stderr_text(&output));
    let bundle = fs::read_to_string(directory.path().join("out.luau")).expect("the bundle");
    assert!(bundle.contains("return \"luck\""), "{bundle}");
}

/// The mode makes the mismatch an error, which the module the alias names
/// gives the type of.
#[test]
fn check_takes_the_mode_and_aliases_of_a_config_luau() {
    let directory = project(".config.luau", ALIASES);

    let output = run(directory.path(), &["check", "main.luau"]);

    let stderr = stderr_text(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stderr.matches("[E015]").count(), 1, "{stderr}");
    assert!(stderr.contains("Expected this to be 'number', but got 'string'"), "{stderr}");
    assert!(stderr.contains("main.luau:2:23"), "{stderr}");
}

#[test]
fn a_directory_with_both_configs_is_an_error() {
    let directory = project(".config.luau", ALIASES);
    write_file(directory.path(), ".luaurc", r#"{ "aliases": { "lib": "./lib" } }"#);

    let checked = run(directory.path(), &["check", "main.luau"]);
    let check_stderr = stderr_text(&checked);
    assert_eq!(checked.status.code(), Some(1), "{check_stderr}");
    assert!(
        check_stderr
            .contains("[E014] error: invalid config: Both .luaurc and .config.luau files exist"),
        "{check_stderr}"
    );

    let built = run(directory.path(), &["build", "--quiet"]);
    let build_stderr = stderr_text(&built);
    assert_eq!(built.status.code(), Some(1), "{build_stderr}");
    assert!(build_stderr.contains("module not found: `@lib/name`"), "{build_stderr}");
    assert!(
        build_stderr.contains("which has both a `.luaurc` and a `.config.luau`"),
        "{build_stderr}"
    );
}

#[test]
fn a_config_luau_that_errors_is_reported_in_luau_s_words() {
    let directory = project(".config.luau", "local aliases = nil\nreturn aliases.lib\n");

    let checked = run(directory.path(), &["check", "main.luau"]);
    let check_stderr = stderr_text(&checked);
    assert_eq!(checked.status.code(), Some(1), "{check_stderr}");
    assert!(
        check_stderr.contains("invalid config: config:2: attempt to index nil with 'lib'"),
        "{check_stderr}"
    );
    assert!(check_stderr.contains(".config.luau:2:1"), "{check_stderr}");

    let built = run(directory.path(), &["build", "--quiet"]);
    let build_stderr = stderr_text(&built);
    assert_eq!(built.status.code(), Some(1), "{build_stderr}");
    assert!(build_stderr.contains(".config.luau`, which doesn't load"), "{build_stderr}");
}

#[test]
fn a_config_luau_runs_once_for_every_module_below_it() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", "{}");
    write_file(directory.path(), ".config.luau", "while true do end\n");
    for name in ["a.luau", "b.luau", "src/c.luau", "src/d.luau"] {
        write_file(directory.path(), name, "return 1\n");
    }

    let started = Instant::now();
    let checked = run(directory.path(), &["check"]);
    let elapsed = started.elapsed();

    let stderr = stderr_text(&checked);
    assert_eq!(checked.status.code(), Some(1), "{stderr}");
    assert_eq!(stderr.matches("[E014]").count(), 1, "{stderr}");
    // Each run takes a second, and Analysis asks for each module's config
    // twice.
    assert!(elapsed < Duration::from_secs(4), "it took {elapsed:?}");
}

#[test]
fn a_config_luau_that_never_returns_is_stopped_and_reported() {
    let directory = project(".config.luau", "while true do end\n");

    let checked = run(directory.path(), &["check", "main.luau"]);
    let check_stderr = stderr_text(&checked);
    assert_eq!(checked.status.code(), Some(1), "{check_stderr}");
    assert!(
        check_stderr
            .contains("invalid config: config:1: Typeinfer failed to complete in allotted time"),
        "{check_stderr}"
    );
    assert!(check_stderr.contains(".config.luau:1:1"), "{check_stderr}");

    let built = run(directory.path(), &["build", "--quiet"]);
    let build_stderr = stderr_text(&built);
    assert_eq!(built.status.code(), Some(1), "{build_stderr}");
    assert!(build_stderr.contains(".config.luau`, which doesn't load"), "{build_stderr}");
}
