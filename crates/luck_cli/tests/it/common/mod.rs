use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;

pub(crate) const TWO_MODULE_ENTRY: &str = "\
local h = require(\"helper\")
print(h)
";
pub(crate) const HELPER_MODULE: &str = "return 1\n";

/// A `luck` command run in `directory`, so config discovery starts in a temp
/// tree instead of this repository.
pub(crate) fn luck_in(directory: &Path) -> Command {
    let mut command = cargo_bin_cmd!("luck");
    command.current_dir(directory);
    command
}

pub(crate) fn run(directory: &Path, args: &[&str]) -> Output {
    luck_in(directory).args(args).output().expect("luck runs")
}

pub(crate) fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(crate) fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Creates the directories `name` sits in as well.
pub(crate) fn write_file(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    fs::create_dir_all(path.parent().expect("a file has a parent")).expect("create directories");
    fs::write(&path, contents).expect("write temp source");
    path
}
