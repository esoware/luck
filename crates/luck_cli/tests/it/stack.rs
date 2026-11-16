//! Every command has to get through input nested as deep as Lua 5.4 and Luau
//! take it.
//!
//! The binary runs each command on the stack its thread starts with, which is
//! 1 MB for a Windows main thread.

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
#[cfg_attr(debug_assertions, ignore = "only a release build's frames fit, so `just stack` runs it")]
fn every_command_fits_the_smallest_stack() {
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
