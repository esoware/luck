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

/// The E015s `luck check` reports for `main`, a strict file of a Roblox
/// project.
fn roblox_type_errors(main: &str) -> String {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", r#"{ "definitions": ["@roblox"] }"#);
    write_file(directory.path(), "main.luau", main);
    stderr_text(&run(directory.path(), &["check", "main.luau"]))
}

#[test]
fn check_types_instance_new_as_the_class_it_names() {
    let stderr = roblox_type_errors(
        "\
--!strict
local part: Part = Instance.new(\"Part\")
local anchored: number = part.Anchored
return anchored
",
    );

    assert_eq!(stderr.matches("[E015]").count(), 1, "{stderr}");
    assert!(stderr.contains("Expected this to be 'number', but got 'boolean'"), "{stderr}");
    assert!(stderr.contains("main.luau:3:26"), "{stderr}");
}

#[test]
fn check_types_get_service_as_the_service_it_names() {
    let stderr = roblox_type_errors(
        "\
--!strict
local players: Players = game:GetService(\"Players\")
local count: string = #players:GetPlayers()
return count
",
    );

    assert_eq!(stderr.matches("[E015]").count(), 1, "{stderr}");
    assert!(stderr.contains("Expected this to be 'string', but got 'number'"), "{stderr}");
}

#[test]
fn check_reports_a_class_name_roblox_has_no_class_for() {
    let stderr = roblox_type_errors("--!strict\nreturn Instance.new(\"Parts\")\n");

    assert_eq!(stderr.matches("[E015]").count(), 1, "{stderr}");
    assert!(stderr.contains("Unknown type 'Parts'"), "{stderr}");
}

/// The Creator Documentation calls `timeOut` optional, so a strict file can
/// leave it out.
#[test]
fn check_takes_wait_for_child_without_a_time_out() {
    let stderr = roblox_type_errors(
        "--!strict
local child: Instance = script:WaitForChild(\"x\")
return child
",
    );

    assert!(!stderr.contains("[E015]"), "{stderr}");
}

#[test]
fn check_takes_a_class_name_that_isnt_a_literal() {
    let stderr = roblox_type_errors(
        "\
--!strict
local function make(className: string): Instance
    return Instance.new(className)
end
return make
",
    );

    assert!(!stderr.contains("[E015]"), "{stderr}");
}

#[test]
fn check_takes_enum_types_in_annotations() {
    let stderr = roblox_type_errors(
        "\
--!strict
local material: Enum.Material = Enum.Material.Plastic
local key: Enum.KeyCode = Enum.Material.Wood
return material, key
",
    );

    assert_eq!(stderr.matches("[E015]").count(), 1, "{stderr}");
    assert!(
        stderr.contains("Expected this to be\n\t'Enum.KeyCode'\nbut got\n\t'Enum.Material'"),
        "{stderr}"
    );
    assert!(stderr.contains("main.luau:3:27"), "{stderr}");
}

/// `GetDebugId` needs `PluginSecurity`, so a script at `None` can't reach it.
#[test]
fn check_gives_a_roblox_project_the_api_of_its_security_level() {
    const DEBUG_ID: &str = "--!strict\nreturn game:GetDebugId()\n";
    let script = TempDir::new().unwrap();
    write_file(script.path(), "luck.json", r#"{ "definitions": ["@roblox"] }"#);
    write_file(script.path(), "main.luau", DEBUG_ID);
    let plugin = TempDir::new().unwrap();
    write_file(
        plugin.path(),
        "luck.json",
        r#"{ "definitions": ["@roblox"], "security": "PluginSecurity" }"#,
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

/// A project's definition files load after Roblox's, so one can name a
/// Roblox class, and checking `.` leaves the definition file among the
/// sources out of the modules it checks.
#[test]
fn check_loads_a_project_s_definition_files_after_roblox_s() {
    let directory = TempDir::new().unwrap();
    write_file(
        directory.path(),
        "luck.json",
        r#"{ "definitions": ["@roblox", "types/app.d.luau"] }"#,
    );
    write_file(
        directory.path(),
        "types/app.d.luau",
        "declare function spawnPart(name: string): Part\n",
    );
    let main = "\
--!strict
local anchored: number = spawnPart(\"floor\").Anchored
return anchored
";
    write_file(directory.path(), "main.luau", main);

    let output = run(directory.path(), &["check", "."]);

    let stderr = stderr_text(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stderr.matches("[E015]").count(), 1, "{stderr}");
    assert!(stderr.contains("Expected this to be 'number', but got 'boolean'"), "{stderr}");
    assert!(stderr.contains("main.luau:2:26"), "{stderr}");
}

/// The checker loads the project's other definitions without it, and checks
/// every module.
#[test]
fn check_reports_luau_s_errors_in_a_definition_file_at_it() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", r#"{ "definitions": ["app.d.luau"] }"#);
    write_file(directory.path(), "app.d.luau", "declare function greet(name: Missing): string\n");
    write_file(directory.path(), "main.luau", MISMATCH);

    let output = run(directory.path(), &["check", "main.luau"]);

    let stderr = stderr_text(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(stderr.matches("[E015]").count(), 2, "{stderr}");
    assert!(stderr.contains("app.d.luau:1:30"), "{stderr}");
    assert!(stderr.contains("main.luau:2:23"), "{stderr}");
}

/// Reported once, by luck's parser where `check` reads the file and by
/// Luau's otherwise, while the project's other definitions load.
#[test]
fn check_reports_a_definition_file_that_does_not_parse_and_checks_the_rest() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "luck.json", r#"{ "definitions": ["@roblox", "app.d.luau"] }"#);
    write_file(directory.path(), "app.d.luau", "declare function greet(name: string: string\n");
    let main = "\
--!strict
local anchored: number = Instance.new(\"Part\").Anchored
return anchored
";
    write_file(directory.path(), "main.luau", main);

    for path in [".", "main.luau"] {
        let output = run(directory.path(), &["check", path]);

        let stderr = stderr_text(&output);
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert_eq!(stderr.matches("[E008]").count(), 1, "{stderr}");
        assert!(stderr.contains("app.d.luau:1:"), "{stderr}");
        assert!(stderr.contains("Expected this to be 'number', but got 'boolean'"), "{stderr}");
    }
}
