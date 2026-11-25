//! The binary runs each command on threads of `luck_ast::MIN_STACK_BYTES`,
//! so every command has to get through input nested to the parser's cap on
//! them, and through runs and lists far longer than that cap.

use assert_cmd::assert::Assert;
use luck_ast::MAX_NESTING_DEPTH;
use luck_testgen::sources_at_the_nesting_cap;
use tempfile::TempDir;

use crate::common::{luck_in, run, stderr_text, write_file};

/// 0 when the command found nothing, 1 when it found problems, and anything
/// else, such as a stack overflow's exit code, a failure.
fn assert_ran(output: &Assert) {
    let code = output.get_output().status.code();
    assert!(matches!(code, Some(0 | 1)), "{:?}", output.get_output());
}

#[test]
fn every_command_gets_through_input_nested_to_the_cap() {
    let directory = TempDir::new().expect("directory");
    write_file(directory.path(), "luck.json", "{}");
    let mut files = Vec::new();
    for (i, source) in sources_at_the_nesting_cap().into_iter().enumerate() {
        let name = format!("src/deep{i}.luau");
        write_file(directory.path(), &name, &source);
        files.push(name);
    }
    for command in [&["lint", "src"][..], &["fmt", "--check", "src"], &["check", "src"]] {
        assert_ran(&luck_in(directory.path()).args(command).assert());
    }
    for file in &files {
        for command in ["minify", "bundle"] {
            assert_ran(&luck_in(directory.path()).args([command, file.as_str()]).assert());
        }
    }
}

/// One run of each kind, each far past the nesting cap. A run is one level
/// of nesting however long it is.
fn long_runs_project() -> TempDir {
    const LINKS: usize = 100_000;
    let source = format!(
        "\
local a = ...
local sum = a{}
local text = a{}
local either = a{}
local field = a{}
a{} = a{}
a{}
return sum, text, either, field
",
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

#[test]
fn lint_survives_long_runs() {
    let directory = long_runs_project();

    let output = run(directory.path(), &["lint", "main.lua"]);

    assert_eq!(
        output.status.code(),
        Some(0),
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
        Some(0),
        "`minify` must finish instead of overflowing its stack:\n{}",
        stderr_text(&output)
    );
}

#[test]
fn bundle_survives_long_runs() {
    let directory = long_runs_project();

    let output = run(directory.path(), &["bundle", "main.lua", "-o", "bundle.lua"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "`bundle` must finish instead of overflowing its stack:\n{}",
        stderr_text(&output)
    );
}

#[test]
fn fmt_survives_long_runs() {
    let directory = long_runs_project();

    let output = run(directory.path(), &["fmt", "main.lua"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "`fmt` must finish instead of overflowing its stack:\n{}",
        stderr_text(&output)
    );
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
        "\
local a = ...
if a == 0 then a(){}
end
-- luck: allow(duplicate_conditions)
if false then a(){}
elseif a then a()
end
local t = {{{}}}
{}return t
",
        numbered("\nelseif a == # then a()"),
        "\nelseif false then a()".repeat(ITEMS),
        numbered("k# = a, "),
        numbered("a(#)\n"),
    );
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", &source);
    let params = format!("local function f(_{}) return _ end\nreturn f\n", numbered(", _#"));
    write_file(directory.path(), "params.lua", &params);
    directory
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
        &["lint", "params.lua"],
        &["minify", "params.lua", "-o", "minified_params.lua"],
        &["fmt", "params.lua"],
    ] {
        let output = run(directory.path(), args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "`{}` must finish instead of overflowing its stack or going quadratic:\n{}",
            args[0],
            stderr_text(&output)
        );
    }
}

/// A project whose `deep.lua` holds `1` inside `tables` table constructors.
/// Its own block is a level, and its wrapper in a bundle adds two more.
fn deep_module_project(tables: usize) -> TempDir {
    let directory = TempDir::new().unwrap();
    write_file(directory.path(), "main.lua", "return require(\"./deep\")\n");
    let deep = format!("return {}1{}\n", "{".repeat(tables), "}".repeat(tables));
    write_file(directory.path(), "deep.lua", &deep);
    directory
}

#[test]
fn bundle_at_the_nesting_cap_reads_back() {
    // That bundle nests exactly `MAX_NESTING_DEPTH` levels deep.
    let directory = deep_module_project(MAX_NESTING_DEPTH - 3);

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
            Some(0),
            "`{}`:\n{}",
            args.join(" "),
            stderr_text(&output)
        );
    }
}

#[test]
fn bundle_reports_a_module_too_deep_to_wrap_in_that_module() {
    let directory = deep_module_project(MAX_NESTING_DEPTH - 1);
    // The first table the wrapper pushes past the cap follows `return ` and
    // the tables before it.
    let first_too_deep = format!("deep.lua:1:{} ", "return ".len() + MAX_NESTING_DEPTH - 2);

    for args in [&["bundle", "main.lua"][..], &["bundle", "main.lua", "--minify"]] {
        let output = run(directory.path(), args);
        let stderr = stderr_text(&output);
        assert_eq!(output.status.code(), Some(1), "`{}`:\n{stderr}", args.join(" "));
        assert!(stderr.contains("E013"), "{stderr}");
        assert!(stderr.contains(&first_too_deep), "{stderr}");
        assert!(!stderr.contains("main.lua:"), "{stderr}");
    }
}
