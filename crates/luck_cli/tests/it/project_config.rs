use std::fs;

use predicates::prelude::*;
use tempfile::TempDir;

use crate::common::{luck_in, write_file};

fn project_with_generated_code(config: &str) -> TempDir {
    let project = TempDir::new().expect("project");
    fs::create_dir_all(project.path().join("src")).expect("create src");
    fs::create_dir_all(project.path().join("gen")).expect("create gen");
    write_file(project.path(), "luck.json", config);
    write_file(project.path(), "src/main.lua", "print(1)\n");
    write_file(project.path(), "gen/bad.lua", "local = 1\n");
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
    let project = project_with_generated_code(r#"{"include":["src/**/*.lua"]}"#);
    luck_in(project.path())
        .args(["check", "-c", "luck.json"])
        .assert()
        .success()
        .stderr(predicate::str::contains("ok: 1 file(s) checked"));
}

#[test]
fn bare_config_name_loads_a_root_config_with_extends() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "base.json", r#"{"lua":"lua54"}"#);
    write_file(project.path(), "luck.json", r#"{"root":true,"extends":["base.json"]}"#);
    write_file(project.path(), "main.lua", "print(1)\n");
    luck_in(project.path()).args(["check", "-c", "luck.json"]).assert().success();
}

/// `bundle` and `graph` resolve a project's modules as `build` does, from its
/// root and `search_paths`, however the entry is named.
#[test]
fn bundle_and_graph_resolve_through_the_project_s_search_paths() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "luck.json", r#"{"search_paths":["src/?.lua","lib/?.lua"]}"#);
    write_file(project.path(), "src/main.lua", "return require(\"util\").answer\n");
    write_file(project.path(), "lib/util.lua", "return { answer = 42 }\n");
    for command in ["bundle", "graph"] {
        luck_in(&project.path().join("src")).args([command, "main.lua"]).assert().success();
    }
    luck_in(project.path())
        .args(["bundle", "src/main.lua", "-s", "src/?.lua"])
        .assert()
        .code(1i32)
        .stderr(predicate::str::contains("module not found: `util`"));
}

#[test]
fn an_override_layers_its_lint_settings_over_the_files_it_matches() {
    let project = TempDir::new().expect("project");
    write_file(
        project.path(),
        "luck.json",
        r#"{"overrides":[{"files":["spec/**"],"lint":{"rules":{"unused_variable":"off"}}}]}"#,
    );
    write_file(project.path(), "src/a.lua", "local unused = 1\n");
    write_file(project.path(), "spec/a_spec.lua", "local unused = 1\n");
    luck_in(project.path()).args(["lint", "--deny-warnings", "spec"]).assert().success();
    luck_in(project.path()).args(["lint", "--deny-warnings", "src"]).assert().code(1i32);
    luck_in(project.path())
        .args(["lint", "--deny-warnings", "-W", "unused_variable", "spec"])
        .assert()
        .code(1i32);
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
    write_file(project.path(), "a.lua", source);
    write_file(project.path(), "vendor/b.lua", source);
    luck_in(project.path()).arg("fmt").assert().success();
    let formatted = |name: &str| fs::read_to_string(project.path().join(name)).expect("read");
    assert!(formatted("a.lua").contains("\n  print(1)"), "{}", formatted("a.lua"));
    assert!(formatted("vendor/b.lua").contains("\n    print(1)"), "{}", formatted("vendor/b.lua"));
}

#[test]
fn print_config_shows_the_overrides_a_file_matches() {
    let project = TempDir::new().expect("project");
    write_file(
        project.path(),
        "luck.json",
        r#"{"overrides":[{"files":["spec/**"],"lint":{"extra_globals":["describe"]}}]}"#,
    );
    write_file(project.path(), "spec/a_spec.lua", "");
    luck_in(project.path())
        .args(["lint", "--print-config", "spec/a_spec.lua"])
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
    write_file(project.path(), "a.lua", "print(1)\n");
    luck_in(project.path())
        .arg("lint")
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("E014"))
        .stderr(predicate::str::contains("did you mean `unused_variable`?"))
        .stderr(predicate::str::contains("luck.json:2:24"));
}

#[test]
fn lint_reads_the_std_definitions_a_config_names() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "luck.json", r#"{ "lua": "lua51", "std": ["love.toml"] }"#);
    write_file(
        project.path(),
        "love.toml",
        r#"
[globals.love]
kind = "namespace"

[globals.love.members.graphics]
kind = "namespace"

[globals.love.members.graphics.members.print]
kind = "function"
params = [{ kind = "string" }, { kind = "number" }, { kind = "number" }]

[globals.love.members.draw]
kind = "function"
is_read_only = false
"#,
    );
    write_file(
        project.path(),
        "main.lua",
        "\
function love.draw()
  love.graphics.print('hello', 0, 0)
end
",
    );
    luck_in(project.path()).arg("lint").assert().success();
    write_file(project.path(), "main.lua", "love.graphics.print('hello')\n");
    luck_in(project.path())
        .arg("lint")
        .assert()
        .failure()
        .stderr(predicate::str::contains("`love.graphics.print` requires at least 3 argument(s)"));
}

#[test]
fn a_std_definition_error_names_the_place_in_it() {
    let project = TempDir::new().expect("project");
    write_file(project.path(), "luck.json", r#"{ "std": ["love.toml"] }"#);
    write_file(
        project.path(),
        "love.toml",
        r#"[globals.love]
kind = "namespace"
shape = 1
"#,
    );
    write_file(project.path(), "a.lua", "print(1)\n");
    luck_in(project.path())
        .arg("lint")
        .assert()
        .code(2i32)
        .stderr(predicate::str::contains("E014"))
        .stderr(predicate::str::contains("love.toml"));
}
