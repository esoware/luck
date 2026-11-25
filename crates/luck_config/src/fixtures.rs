use std::fs;

use serde::Deserialize as _;
use serde::de::IntoDeserializer as _;
use serde::de::value::Error;
use tempfile::TempDir;

use crate::config::LuckConfig;
use crate::glob::Glob;
use crate::parse::parse;

pub(crate) fn project_with(files: &[(&str, &str)]) -> TempDir {
    let project = TempDir::new().expect("create temp dir");
    for (relative_path, contents) in files {
        let path = project.path().join(relative_path);
        fs::create_dir_all(path.parent().expect("a file has a parent")).expect("create parents");
        fs::write(&path, contents).expect("write file");
    }
    project
}

pub(crate) fn glob(pattern: &str) -> Result<Glob, String> {
    Glob::deserialize(pattern.into_deserializer()).map_err(|e: Error| e.to_string())
}

pub(crate) fn parsed(json: &str) -> LuckConfig {
    parse(json).expect("config parses")
}
