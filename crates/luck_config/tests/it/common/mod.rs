use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::{env, fs};

use luck_config::Project;
use luck_diagnostics::Diagnostic;
use luck_resolver::FileSystemOs;
use tempfile::TempDir;

pub(crate) fn project_with(files: &[(&str, &str)]) -> TempDir {
    let project = TempDir::new().expect("create temp dir");
    for (relative_path, contents) in files {
        let path = project.path().join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directories");
        }
        fs::write(&path, contents).expect("write file");
    }
    project
}

/// `files` on disk, and the project of their `luck.json`.
pub(crate) fn loaded(files: &[(&str, &str)]) -> (TempDir, Project) {
    let directory = project_with(files);
    let project =
        Project::load(&directory.path().join("luck.json"), FileSystemOs).expect("config loads");
    (directory, project)
}

/// The one problem that stops the config at `config_path` from loading,
/// and the file it's in.
pub(crate) fn load_error(config_path: &Path) -> (PathBuf, Diagnostic) {
    let error = Project::load(config_path, FileSystemOs).expect_err("the config must not load");
    let [diag] = <[Diagnostic; 1]>::try_from(error.diags).expect("one problem");
    (error.path, diag)
}

/// nextest gives each test its own process, so this can't race another test.
pub(crate) fn in_directory<Output>(directory: &Path, body: impl FnOnce() -> Output) -> Output {
    let original = env::current_dir().expect("read working directory");
    env::set_current_dir(directory).expect("enter directory");
    let output = body();
    env::set_current_dir(original).expect("restore working directory");
    output
}

pub(crate) fn finishes_within<Body, Output>(limit: Duration, body: Body) -> bool
where
    Body: FnOnce() -> Output + Send + 'static,
    Output: Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || sender.send(body()));
    receiver.recv_timeout(limit).is_ok()
}
