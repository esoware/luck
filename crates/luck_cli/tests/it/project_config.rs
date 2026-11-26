use std::fs;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{HELPER_MODULE, TWO_MODULE_ENTRY, luck_in, run, stderr_text, write_file};

fn project_with_generated_code(config: &str) -> TempDir {
    let project = TempDir::new().expect("project");
    fs::create_dir_all(project.path().join("src")).expect("create src");
    fs::create_dir_all(project.path().join("gen")).expect("create gen");
    write_file(project.path(), "luck.json", config);
    write_file(project.path(), "src/main.luau", "print(1)\n");
    write_file(project.path(), "gen/bad.luau", "local = 1\n");
    project
}

#[test]
fn bare_config_name_applies_a_relative_exclude() {
    let project = project_with_generated_code(r#"{"exclude":["gen/**"]}"#);
    luck_in(project.path())
        .args(["check", "-c", "luck.json"])
        .assert()
        .success()
        .stderr(predicate::str::contains("ok: 1 file(s) checked"));
}

#[test]
fn bare_config_name_applies_a_relative_include() {
    let project = project_with_generated_code(r#"{"include":["src/**/*.luau"]}"#);
    luck_in(project.path())
        .args(["check", "-c", "luck.json"])
        .assert()
        .success()
        .stderr(predicate::str::contains("ok: 1 file(s) checked"));
}

#[test]
fn bare_config_name_loads_a_root_config_with_extends() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "base.json", r#"{"minify":false}"#);
    write_file(project.path(), "luck.json", r#"{"root":true,"extends":["base.json"]}"#);
    write_file(project.path(), "main.luau", "print(1)\n");
    luck_in(project.path()).args(["check", "-c", "luck.json"]).assert().success();
}

/// `bundle` and `graph` resolve a project's modules as `build` does, through
/// the `.luaurc` aliases above the entry, however the entry is named.
#[test]
fn bundle_and_graph_resolve_through_the_project_s_aliases() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "luck.json", "{}");
    write_file(project.path(), ".luaurc", r#"{"aliases":{"lib":"./lib"}}"#);
    write_file(project.path(), "src/main.luau", "return require(\"@lib/util\").answer\n");
    write_file(project.path(), "lib/util.luau", "return { answer = 42 }\n");
    for command in ["bundle", "graph"] {
        luck_in(&project.path().join("src")).args([command, "main.luau"]).assert().success();
        luck_in(project.path()).args([command, "src/main.luau"]).assert().success();
    }
    write_file(project.path(), ".luaurc", "{}");
    luck_in(project.path())
        .args(["bundle", "src/main.luau"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("E004"));
}

#[test]
fn graph_resolves_modules_next_to_the_entry_as_bundle_does() {
    let directory = TempDir::new().unwrap();
    fs::create_dir_all(directory.path().join("sources")).unwrap();
    write_file(directory.path(), "sources/main.luau", TWO_MODULE_ENTRY);
    write_file(directory.path(), "sources/helper.luau", HELPER_MODULE);
    let bundled = run(directory.path(), &["bundle", "sources/main.luau", "-o", "bundle.luau"]);
    assert_eq!(bundled.status.code(), Some(0), "{}", stderr_text(&bundled));

    let output = run(directory.path(), &["graph", "sources/main.luau"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "`graph` must find the modules `bundle` finds for the same entry:\n{}",
        stderr_text(&output)
    );
}

#[test]
fn an_override_layers_its_lint_settings_over_the_files_it_matches() {
    let project = TempDir::new().expect("project");
    write_file(
        project.path(),
        "luck.json",
        r#"{"overrides":[{"files":["spec/**"],"lint":{"rules":{"unused_variable":"off"}}}]}"#,
    );
    write_file(project.path(), "src/a.luau", "local unused = 1\n");
    write_file(project.path(), "spec/a_spec.luau", "local unused = 1\n");
    luck_in(project.path()).args(["lint", "--deny-warnings", "spec"]).assert().success();
    luck_in(project.path()).args(["lint", "--deny-warnings", "src"]).assert().code(1);
    luck_in(project.path())
        .args(["lint", "--deny-warnings", "-W", "unused_variable", "spec"])
        .assert()
        .code(1);
}

#[test]
fn an_override_layers_its_format_settings_over_the_files_it_matches() {
    let project = TempDir::new().expect("project");
    write_file(
        project.path(),
        "luck.json",
        r#"{"format":{"indent_style":"spaces","indent_width":2},
            "overrides":[{"files":["vendor/**"],"format":{"indent_width":4}}]}"#,
    );
    let source = "\
if x then
print(1)
end
";
    write_file(project.path(), "a.luau", source);
    write_file(project.path(), "vendor/b.luau", source);
    luck_in(project.path()).arg("fmt").assert().success();
    let formatted = |name: &str| fs::read_to_string(project.path().join(name)).expect("read");
    assert!(formatted("a.luau").contains("\n  print(1)"), "{}", formatted("a.luau"));
    assert!(
        formatted("vendor/b.luau").contains("\n    print(1)"),
        "{}",
        formatted("vendor/b.luau")
    );
}

#[test]
fn fmt_applies_a_path_scoped_editorconfig_section_from_a_subdirectory() {
    let directory = TempDir::new().unwrap();
    let sources = directory.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    write_file(
        directory.path(),
        ".editorconfig",
        "\
root = true
[sources/*.luau]
indent_style = space
indent_size = 3
",
    );
    let path = write_file(
        &sources,
        "main.luau",
        "\
local function f()
return 1
end
",
    );

    run(&sources, &["fmt", "main.luau"]);
    let formatted = fs::read_to_string(path).unwrap();

    assert!(
        formatted.contains("\n   return 1"),
        "the section matches this file whichever directory runs `fmt`:\n{formatted}"
    );
}

#[test]
fn print_config_shows_the_overrides_a_file_matches() {
    let project = TempDir::new().expect("project");
    write_file(
        project.path(),
        "luck.json",
        r#"{"overrides":[{"files":["spec/**"],"lint":{"extra_globals":["describe"]}}]}"#,
    );
    write_file(project.path(), "spec/a_spec.luau", "");
    luck_in(project.path())
        .args(["lint", "--print-config", "spec/a_spec.luau"])
        .assert()
        .success()
        .stdout(predicate::str::contains("describe"));
    luck_in(project.path())
        .args(["lint", "--print-config"])
        .assert()
        .success()
        .stdout(predicate::str::contains("describe").not());
}

#[test]
fn a_config_error_names_the_place_in_the_file() {
    let project = TempDir::new().expect("project");
    write_file(
        project.path(),
        "luck.json",
        "\
{
  \"lint\": { \"rules\": { \"unsued_variable\": \"warn\" } }
}
",
    );
    write_file(project.path(), "a.luau", "print(1)\n");
    luck_in(project.path())
        .arg("lint")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("E014"))
        .stderr(predicate::str::contains("did you mean `unused_variable`?"))
        .stderr(predicate::str::contains("luck.json:2:24"));
}

/// The definition file sits among the sources, and `lint` reads it as the
/// definition file its name makes it, so its `declare` statements are no
/// syntax error.
#[test]
fn lint_reads_the_definition_files_a_config_names() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "luck.json", r#"{ "definitions": ["@roblox", "app.d.luau"] }"#);
    write_file(
        project.path(),
        "app.d.luau",
        "\
declare app: {
    net: { send: (message: string, port: number, timeout: number) -> () },
    onMessage: () -> (),
}
",
    );
    write_file(
        project.path(),
        "main.luau",
        "\
function app.onMessage()
  app.net.send('hello', 0, 0)
end
print(game)
",
    );
    luck_in(project.path()).arg("lint").assert().success();
    write_file(project.path(), "main.luau", "app.net.send('hello')\n");
    luck_in(project.path())
        .arg("lint")
        .assert()
        .failure()
        .stderr(predicate::str::contains("`app.net.send` requires at least 3 argument(s)"));
}

#[test]
fn a_definition_file_that_does_not_parse_is_reported_where() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "luck.json", r#"{ "definitions": ["app.d.luau"] }"#);
    write_file(project.path(), "app.d.luau", "declare function greet(name: string: string\n");
    write_file(project.path(), "a.luau", "print(1)\n");
    luck_in(project.path())
        .arg("lint")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("E008"))
        .stderr(predicate::str::contains("app.d.luau:1:"));
}
