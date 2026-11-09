use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;

/// A `luck` command run in `directory`, so config discovery starts in a temp
/// tree instead of this repository.
pub(crate) fn luck_in(directory: &Path) -> Command {
    let mut command = cargo_bin_cmd!("luck");
    command.current_dir(directory);
    command
}

pub(crate) fn write_file(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, contents).expect("write temp source");
    path
}
