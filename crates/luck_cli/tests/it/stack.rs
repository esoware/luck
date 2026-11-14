//! The binary runs each command on the stack its thread starts with, which is
//! 1 MB for a Windows main thread, so every command has to get through input
//! nested to the parser's cap on it.

use assert_cmd::assert::Assert;
use luck_testgen::sources_at_the_nesting_cap;
use tempfile::TempDir;

use crate::common::{luck_in, write_file};

/// 0 when the command found nothing, 1 when it found problems, and anything
/// else, such as a stack overflow's exit code, a failure.
fn assert_ran(output: &Assert) {
    let code = output.get_output().status.code();
    assert!(matches!(code, Some(0i32 | 1i32)), "{:?}", output.get_output());
}

#[test]
fn every_command_gets_through_input_nested_to_the_cap() {
    let directory = TempDir::new().expect("directory");
    write_file(directory.path(), "luck.json", r#"{"lua": "lua54", "luau": "luau"}"#);
    let mut files = Vec::new();
    for (i, (version, source)) in sources_at_the_nesting_cap().into_iter().enumerate() {
        let extension = if version.is_luau() { "luau" } else { "lua" };
        let name = format!("src/deep{i}.{extension}");
        write_file(directory.path(), &name, &source);
        files.push(name);
    }
    for command in [&["lint", "src"][..], &["fmt", "--check", "src"], &["check", "src"]] {
        assert_ran(&luck_in(directory.path()).args(command).assert());
    }
    for file in &files {
        assert_ran(&luck_in(directory.path()).args(["minify", file.as_str()]).assert());
    }
}
