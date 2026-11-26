use tempfile::TempDir;

use crate::common::{run, stderr_text, write_file};

const MISMATCH: &str = "\
--!strict
local count: number = \"one\"
return count
";

#[test]
fn check_reports_a_type_error_in_luaus_words() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.luau", MISMATCH);

    let output = run(directory.path(), &["check", "main.luau"]);

    let stderr = stderr_text(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("[E015]"), "{stderr}");
    assert!(stderr.contains("Expected this to be 'number', but got 'string'"), "{stderr}");
    assert!(stderr.contains("main.luau:2:23"), "{stderr}");
}

/// A require resolves as `luck build` resolves it, `.luaurc` aliases
/// included, so a module's types reach the one requiring it.
#[test]
fn check_follows_requires_through_luaurc_aliases() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), ".luaurc", r#"{ "aliases": { "lib": "./lib" } }"#);
    write_file(directory.path(), "lib/name.luau", "return \"luck\"\n");
    let main = "\
--!strict
local name = require(\"@lib/name\")
local count: number = name
return count
";
    write_file(directory.path(), "main.luau", main);

    let output = run(directory.path(), &["check", "main.luau"]);

    let stderr = stderr_text(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("main.luau:3:23"), "{stderr}");
}

#[test]
fn check_takes_the_mode_of_the_luaurc_above_a_file() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "loose.luau", "local count: number = \"one\"\nreturn count\n");
    write_file(directory.path(), "strict/.luaurc", r#"{ "languageMode": "strict" }"#);
    write_file(
        directory.path(),
        "strict/main.luau",
        "local count: number = \"one\"\nreturn count\n",
    );

    let output = run(directory.path(), &["check", "."]);

    let stderr = stderr_text(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stderr.matches("[E015]").count(), 1, "{stderr}");
    assert!(stderr.contains("main.luau:1:23"), "{stderr}");
}

/// Once, at the line Luau names, however many files it governs.
#[test]
fn check_reports_a_luaurc_luau_cannot_read() {
    let directory = TempDir::new().unwrap();
    let luaurc = r#"{
  "languageMode": "strict"
  "globals": []
}
"#;
    write_file(directory.path(), ".luaurc", luaurc);
    write_file(directory.path(), "a.luau", "return 1\n");
    write_file(directory.path(), "b.luau", "return 2\n");

    let output = run(directory.path(), &["check", "."]);

    let stderr = stderr_text(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stderr.matches("[E014]").count(), 1, "{stderr}");
    assert!(
        stderr.contains(r#"invalid config: Expected ',' or '}' at line 3, got "globals" instead"#),
        "{stderr}"
    );
    assert!(stderr.contains(".luaurc:3:1"), "{stderr}");
}

/// luck's own parser reports a syntax error, and Luau's checker doesn't
/// report it again.
#[test]
fn check_reports_a_syntax_error_once() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "broken.luau", "local x = 1 +\n");

    let output = run(directory.path(), &["check", "broken.luau"]);

    let stderr = stderr_text(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stderr.matches("[E008]").count(), 1, "{stderr}");
}

#[test]
fn check_types_a_roblox_project_with_robloxs_api() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", r#"{ "variant": "roblox" }"#);
    let main = "\
--!strict
local camera: Camera = workspace.CurrentCamera
local name: number = camera.Name
return name
";
    write_file(directory.path(), "main.luau", main);

    let output = run(directory.path(), &["check", "main.luau"]);

    let stderr = stderr_text(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stderr.matches("[E015]").count(), 1, "{stderr}");
    assert!(stderr.contains("Expected this to be 'number', but got 'string'"), "{stderr}");
    assert!(stderr.contains("main.luau:3:22"), "{stderr}");
}

/// `GetDebugId` needs `PluginSecurity`, so a script at `None` can't reach it.
#[test]
fn check_gives_a_roblox_project_the_api_of_its_security_level() {
    const DEBUG_ID: &str = "--!strict\nreturn game:GetDebugId()\n";
    let script = TempDir::new().unwrap();
    write_file(script.path(), "luck.json", r#"{ "variant": "roblox" }"#);
    write_file(script.path(), "main.luau", DEBUG_ID);
    let plugin = TempDir::new().unwrap();
    write_file(
        plugin.path(),
        "luck.json",
        r#"{ "variant": "roblox", "security": "PluginSecurity" }"#,
    );
    write_file(plugin.path(), "main.luau", DEBUG_ID);

    let at_none = run(script.path(), &["check", "main.luau"]);
    let at_plugin = run(plugin.path(), &["check", "main.luau"]);

    let stderr = stderr_text(&at_none);
    assert_eq!(at_none.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("[E015]"), "{stderr}");
    assert!(stderr.contains("Key 'GetDebugId' not found in external type 'DataModel'"), "{stderr}");
    assert_eq!(at_plugin.status.code(), Some(0), "{}", stderr_text(&at_plugin));
}
