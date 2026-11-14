use std::fs;
use std::path::Path;
use std::process::Output;

use tempfile::TempDir;

use crate::common::{luck_in, write_file};

const TWO_MODULE_ENTRY: &str = "local h = require(\"helper\")\nprint(h)\n";
const HELPER_MODULE: &str = "return 1\n";

fn run(directory: &Path, args: &[&str]) -> Output {
    luck_in(directory).args(args).output().expect("luck runs")
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn make_read_only(path: &Path) {
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).expect("set read-only");
}

fn two_module_project() -> TempDir {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", TWO_MODULE_ENTRY);
    write_file(directory.path(), "helper.lua", HELPER_MODULE);
    directory
}

/// One run of each kind, each far past the nesting cap. A run is one level
/// of nesting however long it is.
fn long_runs_project() -> TempDir {
    const LINKS: usize = 100_000;
    let source = format!(
        "local a = ...\nlocal sum = a{}\nlocal text = a{}\nlocal either = a{}\n\
         local field = a{}\na{} = a{}\na{}\nreturn sum, text, either, field\n",
        "+a".repeat(LINKS),
        "..a".repeat(LINKS),
        " and a or a".repeat(LINKS),
        ".b".repeat(LINKS),
        "[1]".repeat(LINKS),
        "()".repeat(LINKS),
        ":m()".repeat(LINKS),
    );
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", &source);
    directory
}

/// One sibling list of each kind. A list is one level of nesting however long
/// it is, and 20,000 `elseif`s overflowed `minify`'s stack when it recursed
/// per clause.
fn long_lists_project() -> TempDir {
    const ITEMS: usize = 20_000;
    let numbered = |template: &str| -> String {
        (0..ITEMS).map(|i| template.replace('#', &i.to_string())).collect()
    };
    let source = format!(
        "local a = ...\nif a == 0 then a(){}\nend\n\
         -- luck: allow(duplicate_conditions)\nif false then a(){}\nelseif a then a()\nend\n\
         local function f(_{}) return a end\nlocal t = {{{}}}\n{}return f, t\n",
        numbered("\nelseif a == # then a()"),
        "\nelseif false then a()".repeat(ITEMS),
        numbered(", _#"),
        numbered("k# = a, "),
        numbered("goto l#\n::l#::\n"),
    );
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", &source);
    directory
}

#[cfg(windows)]
fn set_directory_listing(directory: &Path, is_allowed: bool) {
    use std::process::Command;

    const EVERYONE: &str = "*S-1-1-0";
    let mut command = Command::new("icacls");
    command.arg(directory);
    if is_allowed {
        command.args(["/remove:d", EVERYONE]);
    } else {
        command.args(["/deny", &format!("{EVERYONE}:(OI)(CI)(RX)")]);
    }
    assert!(command.output().expect("icacls runs").status.success(), "icacls must apply the ACL");
}

#[cfg(unix)]
fn set_directory_listing(directory: &Path, is_allowed: bool) {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = if is_allowed { 0o755 } else { 0o000 };
    fs::set_permissions(directory, fs::Permissions::from_mode(mode)).expect("chmod");
}

#[test]
fn lint_renders_a_span_ending_inside_a_multibyte_char() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "print(string.format(\"%\u{e9}\", 1))\n");

    let output = run(directory.path(), &["lint", "main.lua"]);
    let stderr = stderr_text(&output);

    assert!(
        !stderr.contains("panicked"),
        "a finding that ends inside a multibyte character must render, not panic:\n{stderr}"
    );
    assert_eq!(output.status.code(), Some(0i32), "one warning exits 0:\n{stderr}");
}

#[test]
fn lint_line_numbers_do_not_count_a_form_feed_as_a_newline() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "-- a\u{c}\nreturn missingglobal\n");

    let stderr = stderr_text(&run(directory.path(), &["lint", "main.lua"]));

    assert!(
        stderr.contains("main.lua:2:8"),
        "Lua counts only CR and LF as line breaks, so the finding is on line 2:\n{stderr}"
    );
}

#[test]
fn lint_column_does_not_count_a_bom() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "\u{feff}return missingglobal\n");

    let stderr = stderr_text(&run(directory.path(), &["lint", "main.lua"]));

    assert!(
        stderr.contains("main.lua:1:8"),
        "a BOM is not a column, so the finding is at 1:8:\n{stderr}"
    );
}

#[test]
fn lint_stdin_fix_json_keeps_findings_out_of_the_fixed_source() {
    let directory = TempDir::new().unwrap();

    let output = luck_in(directory.path())
        .args(["lint", "--fix", "--format", "json", "--stdin-filepath", "buffer.lua"])
        .write_stdin("local unused = 1\nreturn missingglobal")
        .output()
        .expect("luck runs");

    assert_eq!(
        stdout_text(&output),
        "local _unused = 1\nreturn missingglobal",
        "`--fix` over stdin writes the fixed source to stdout, and nothing else may follow it"
    );
}

#[test]
fn lint_fix_exits_failure_when_a_file_cannot_be_written() {
    let directory = TempDir::new().unwrap();
    let path = write_file(directory.path(), "main.lua", "local unused = 1\nreturn 1\n");
    make_read_only(&path);

    let output = run(directory.path(), &["lint", "--fix", "main.lua"]);

    assert_eq!(
        output.status.code(),
        Some(1i32),
        "a file that can't be written fails the run:\n{}",
        stderr_text(&output)
    );
}

#[test]
fn lint_counts_a_file_reached_twice_once() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "local unused = 1\nreturn 1\n");

    let output = run(directory.path(), &["lint", ".", "main.lua", "--max-warnings", "1"]);

    assert_eq!(
        output.status.code(),
        Some(0i32),
        "one warning is within `--max-warnings 1` however many arguments reach the file:\n{}",
        stderr_text(&output)
    );
}

#[test]
fn lint_survives_long_runs() {
    let directory = long_runs_project();

    let output = run(directory.path(), &["lint", "main.lua"]);

    assert_eq!(
        output.status.code(),
        Some(0i32),
        "`lint` must finish instead of overflowing its stack:\n{}",
        stderr_text(&output)
    );
}

#[test]
fn minify_survives_long_runs() {
    let directory = long_runs_project();

    let output = run(directory.path(), &["minify", "main.lua", "-o", "minified.lua"]);

    assert_eq!(
        output.status.code(),
        Some(0i32),
        "`minify` must finish instead of overflowing its stack:\n{}",
        stderr_text(&output)
    );
}

#[test]
fn minify_missing_input_exits_usage() {
    let directory = TempDir::new().unwrap();

    let output = run(directory.path(), &["minify", "missing.lua"]);

    assert_eq!(
        output.status.code(),
        Some(2i32),
        "a path that doesn't exist exits 2, as it does for every other command:\n{}",
        stderr_text(&output)
    );
}

#[test]
fn graph_resolves_modules_next_to_the_entry_as_bundle_does() {
    let directory = TempDir::new().unwrap();
    fs::create_dir_all(directory.path().join("sources")).unwrap();
    write_file(directory.path(), "sources/main.lua", TWO_MODULE_ENTRY);
    write_file(directory.path(), "sources/helper.lua", HELPER_MODULE);
    let bundled = run(directory.path(), &["bundle", "sources/main.lua", "-o", "bundle.lua"]);
    assert_eq!(bundled.status.code(), Some(0i32), "{}", stderr_text(&bundled));

    let output = run(directory.path(), &["graph", "sources/main.lua"]);

    assert_eq!(
        output.status.code(),
        Some(0i32),
        "`graph` must find the modules `bundle` finds for the same entry:\n{}",
        stderr_text(&output)
    );
}

#[test]
fn bundle_survives_long_runs() {
    let directory = long_runs_project();

    let output = run(directory.path(), &["bundle", "main.lua", "-o", "bundle.lua"]);

    assert_eq!(
        output.status.code(),
        Some(0i32),
        "`bundle` must finish instead of overflowing its stack:\n{}",
        stderr_text(&output)
    );
}

/// A project whose `deep.lua` holds `1` in `groups` parenthesized groups.
/// Its own block is a level, and its wrapper in a bundle adds two more.
fn deep_module_project(groups: usize) -> TempDir {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "return require(\"deep\")\n");
    let deep = format!("return {}1{}\n", "(".repeat(groups), ")".repeat(groups));
    write_file(directory.path(), "deep.lua", &deep);
    directory
}

#[test]
fn bundle_at_the_nesting_cap_reads_back() {
    // That bundle nests exactly `luck_ast::MAX_NESTING_DEPTH` levels deep.
    let directory = deep_module_project(253);

    for args in [
        &["bundle", "main.lua", "-o", "bundle.lua"][..],
        &["bundle", "main.lua", "--minify", "-o", "minified.lua"],
        &["lint", "bundle.lua", "minified.lua"],
        &["check", "bundle.lua", "minified.lua"],
        &["fmt", "bundle.lua", "minified.lua"],
    ] {
        let output = run(directory.path(), args);
        assert_eq!(
            output.status.code(),
            Some(0i32),
            "`{}`:\n{}",
            args.join(" "),
            stderr_text(&output)
        );
    }
}

#[test]
fn bundle_reports_a_module_too_deep_to_wrap_in_that_module() {
    let directory = deep_module_project(255);

    for args in [&["bundle", "main.lua"][..], &["bundle", "main.lua", "--minify"]] {
        let output = run(directory.path(), args);
        let stderr = stderr_text(&output);
        assert_eq!(output.status.code(), Some(1i32), "`{}`:\n{stderr}", args.join(" "));
        assert!(stderr.contains("E013"), "{stderr}");
        // The 254th group, the first the wrapper pushes past the cap, opens at
        // byte 260 of `deep.lua`.
        assert!(stderr.contains("deep.lua:1:261 "), "{stderr}");
        assert!(!stderr.contains("main.lua:"), "{stderr}");
    }
}

#[test]
fn build_keeps_every_preamble_line_inside_its_comment() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "print(1)\n");
    write_file(
        directory.path(),
        "luck.json",
        r#"{"entry":"main.lua","output":"bundle.lua","preamble":"one\rerror(\"escaped\")\r\ntwo"}"#,
    );

    run(directory.path(), &["build", "--quiet"]);
    let bundle = fs::read_to_string(directory.path().join("bundle.lua")).unwrap();

    assert!(
        bundle.split(['\r', '\n']).all(|line| !line.starts_with("error(")),
        "a short comment ends at a CR, so a preamble line must not hold one:\n{bundle:?}"
    );
    assert!(bundle.starts_with("-- one\n-- error(\"escaped\")\n-- two\n"), "{bundle:?}");
}

#[test]
fn bundle_refuses_to_overwrite_its_entry() {
    let directory = two_module_project();

    run(directory.path(), &["bundle", "main.lua", "-o", "main.lua"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("main.lua")).unwrap(),
        TWO_MODULE_ENTRY,
        "`-o` naming the entry must not destroy the entry's source"
    );
}

#[test]
fn bundle_refuses_to_overwrite_a_dependency() {
    let directory = two_module_project();

    run(directory.path(), &["bundle", "main.lua", "-o", "helper.lua"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("helper.lua")).unwrap(),
        HELPER_MODULE,
        "`-o` naming a bundled module must not destroy the module's source"
    );
}

#[test]
fn bundle_line_map_refuses_to_overwrite_its_entry() {
    let directory = two_module_project();

    run(directory.path(), &["bundle", "main.lua", "-o", "bundle.lua", "--line-map", "main.lua"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("main.lua")).unwrap(),
        TWO_MODULE_ENTRY,
        "`--line-map` naming the entry must not replace its source with JSON"
    );
}

#[test]
fn bundle_rejects_one_path_for_the_output_and_the_line_map() {
    let directory = two_module_project();

    let output =
        run(directory.path(), &["bundle", "main.lua", "-o", "same.lua", "--line-map", "same.lua"]);

    assert!(
        !output.status.success(),
        "one path can't hold both the bundle and its line map, so the run must fail, not lose one"
    );
}

#[test]
fn build_refuses_to_overwrite_its_entry() {
    let directory = two_module_project();
    write_file(directory.path(), "luck.json", r#"{"entry":"main.lua","output":"main.lua"}"#);

    run(directory.path(), &["build", "--quiet"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("main.lua")).unwrap(),
        TWO_MODULE_ENTRY,
        "an `output` equal to the `entry` must not destroy the entry's source"
    );
}

#[test]
fn build_refuses_to_overwrite_a_dependency() {
    let directory = two_module_project();
    write_file(directory.path(), "luck.json", r#"{"entry":"main.lua","output":"helper.lua"}"#);

    run(directory.path(), &["build", "--quiet"]);

    assert_eq!(
        fs::read_to_string(directory.path().join("helper.lua")).unwrap(),
        HELPER_MODULE,
        "an `output` naming a bundled module must not destroy the module's source"
    );
}

#[test]
fn build_rejects_two_entries_writing_one_output() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "a.lua", "return 1\n");
    write_file(directory.path(), "b.lua", "return 2\n");
    write_file(
        directory.path(),
        "luck.json",
        r#"{"entries":[{"entry":"a.lua","output":"same.lua"},{"entry":"b.lua","output":"same.lua"}]}"#,
    );

    let output = run(directory.path(), &["build", "--quiet"]);

    assert!(
        !output.status.success(),
        "two entries sharing an output lose the first bundle, so the build must fail"
    );
}

#[test]
fn build_preamble_stays_after_a_shebang() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "#!/usr/bin/env lua\nprint(1)\n");
    write_file(
        directory.path(),
        "luck.json",
        r#"{"entry":"main.lua","output":"bundle.lua","preamble":"hello"}"#,
    );

    run(directory.path(), &["build", "--quiet"]);
    let bundle = fs::read_to_string(directory.path().join("bundle.lua")).unwrap();

    assert!(
        bundle.starts_with("#!"),
        "Lua only skips a shebang on the first line of a chunk, so it must stay there:\n{bundle}"
    );
}

#[test]
fn build_preamble_leaves_no_bom_inside_the_bundle() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "\u{feff}print(1)\n");
    write_file(
        directory.path(),
        "luck.json",
        r#"{"entry":"main.lua","output":"bundle.lua","preamble":"hello"}"#,
    );

    run(directory.path(), &["build", "--quiet"]);
    let bundle = fs::read_to_string(directory.path().join("bundle.lua")).unwrap();

    assert!(
        !bundle.trim_start_matches('\u{feff}').contains('\u{feff}'),
        "Lua only skips a BOM at the start of a chunk, so none may follow the preamble:\n{bundle}"
    );
}

#[test]
fn build_reports_a_size_increase_without_a_double_minus() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "return 1\n");
    write_file(
        directory.path(),
        "luck.json",
        r#"{"entry":"main.lua","output":"bundle.lua","minify":true,"preamble":"a preamble long enough to outweigh the code"}"#,
    );

    let stderr = stderr_text(&run(directory.path(), &["build"]));

    assert!(!stderr.contains("(--"), "a grown bundle must not report `(--N%)`:\n{stderr}");
}

#[test]
fn build_rejects_release_with_dev() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "return 1\n");
    write_file(
        directory.path(),
        "luck.json",
        r#"{"entry":"main.lua","output":"bundle.lua","profiles":{"dev":{},"release":{}}}"#,
    );

    let output = run(directory.path(), &["build", "--release", "--dev", "--dry-run"]);

    assert_eq!(
        output.status.code(),
        Some(2i32),
        "the shorthands select contradictory profiles, as `--profile` given twice does"
    );
}

#[test]
fn check_fails_when_a_directory_cannot_be_walked() {
    let directory = TempDir::new().unwrap();
    let locked = directory.path().join("locked");
    fs::create_dir_all(&locked).unwrap();
    write_file(&locked, "bad.lua", "local = 1\n");
    write_file(directory.path(), "ok.lua", "return 1\n");

    set_directory_listing(&locked, false);
    let output = run(directory.path(), &["check"]);
    set_directory_listing(&locked, true);

    assert!(
        !output.status.success(),
        "a directory that can't be read leaves files unchecked, so the run must fail:\n{}",
        stderr_text(&output)
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
        "root = true\n[sources/*.lua]\nindent_style = space\nindent_size = 3\n",
    );
    let path = write_file(&sources, "main.lua", "local function f()\nreturn 1\nend\n");

    run(&sources, &["fmt", "main.lua"]);
    let formatted = fs::read_to_string(path).unwrap();

    assert!(
        formatted.contains("\n   return 1"),
        "the section matches this file whichever directory runs `fmt`:\n{formatted}"
    );
}

#[test]
fn fmt_survives_long_runs() {
    let directory = long_runs_project();

    let output = run(directory.path(), &["fmt", "main.lua"]);

    assert_eq!(
        output.status.code(),
        Some(0i32),
        "`fmt` must finish instead of overflowing its stack:\n{}",
        stderr_text(&output)
    );
}

#[test]
fn every_command_survives_long_lists() {
    let directory = long_lists_project();

    // `fmt` rewrites the file, so it goes last.
    for args in [
        &["lint", "main.lua"][..],
        &["minify", "main.lua", "-o", "minified.lua"],
        &["bundle", "main.lua", "-o", "bundle.lua"],
        &["fmt", "main.lua"],
    ] {
        let output = run(directory.path(), args);
        assert_eq!(
            output.status.code(),
            Some(0i32),
            "`{}` must finish instead of overflowing its stack or going quadratic:\n{}",
            args[0],
            stderr_text(&output)
        );
    }
}

#[test]
fn fmt_range_leaves_crlf_outside_the_range_verbatim() {
    let directory = TempDir::new().unwrap();
    let path = write_file(directory.path(), "main.lua", "local    a=1\r\nlocal    b=2\r\n");

    run(directory.path(), &["fmt", "--range-start", "14", "main.lua"]);
    let formatted = fs::read_to_string(path).unwrap();

    assert!(
        formatted.starts_with("local    a=1\r\n"),
        "a statement outside the range is copied verbatim, line ending included:\n{formatted:?}"
    );
}

#[test]
fn init_quiet_prints_no_summary() {
    let directory = TempDir::new().unwrap();

    let output = run(directory.path(), &["init", "--quiet"]);

    assert_eq!(stderr_text(&output), "", "`--quiet` hides summaries");
}

#[test]
fn init_failure_leaves_no_partial_project() {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "src", "a file where the source directory belongs\n");

    let output = run(directory.path(), &["init"]);
    assert_eq!(output.status.code(), Some(1i32), "{}", stderr_text(&output));

    assert!(
        !directory.path().join("luck.json").exists(),
        "a failed `init` must not leave a `luck.json` that blocks the next attempt"
    );
}
