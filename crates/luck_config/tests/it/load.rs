use std::fs;
use std::path::Path;
use std::time::Duration;

use luck_config::{Project, find_config};
use luck_resolver::{FileSystem as _, FileSystemOs};
use tempfile::TempDir;

use crate::common::{finishes_within, in_directory, load_error, project_with};

#[test]
fn root_config_rejects_an_escape_through_a_transitive_extends() {
    let project = project_with(&[
        ("outside.json", r#"{ "lua": "lua51" }"#),
        ("project/base.json", r#"{ "extends": ["../outside.json"] }"#),
        ("project/luck.json", r#"{ "root": true, "extends": ["base.json"] }"#),
    ]);
    let (path, diag) = load_error(&project.path().join("project/luck.json"));
    assert!(path.ends_with("base.json"), "{path:?}");
    assert!(diag.message.contains("outside"), "{diag:?}");
}

#[test]
fn parse_error_in_an_extended_file_names_that_file() {
    let project = project_with(&[
        ("base.json", "{ broken"),
        ("luck.json", r#"{ "extends": ["base.json"] }"#),
    ]);
    let (path, diag) = load_error(&project.path().join("luck.json"));
    assert!(
        path.ends_with("base.json"),
        "the error must name the file that failed to parse, not `luck.json`: {path:?} {diag:?}"
    );
}

#[test]
fn a_config_that_cannot_be_read_is_e010() {
    let project = project_with(&[]);
    let (_, diag) = load_error(&project.path().join("luck.json"));
    assert_eq!(diag.code, "E010");
    assert_eq!(diag.span, None, "a file that can't be read has no place to point at");
}

#[test]
fn repeated_extends_layers_resolve_promptly() {
    const LAYER_COUNT: usize = 40;
    let project = TempDir::new().expect("create temp dir");
    for layer in 0..LAYER_COUNT {
        let body = if layer + 1 == LAYER_COUNT {
            r#"{ "overrides": [{ "files": ["spec/**"] }] }"#.to_owned()
        } else {
            let next = layer + 1;
            format!(r#"{{ "extends": ["layer{next}.json", "layer{next}.json"] }}"#)
        };
        fs::write(project.path().join(format!("layer{layer}.json")), body).expect("write layer");
    }
    let first_layer = project.path().join("layer0.json");
    let is_finished = finishes_within(Duration::from_secs(10), move || Project::load(&first_layer));
    assert!(is_finished, "40 small config files must not take 2^40 file loads to resolve");
}

#[test]
fn long_extends_chain_does_not_overflow_the_stack() {
    const LAYER_COUNT: usize = 5000;
    let project = TempDir::new().expect("create temp dir");
    for layer in 0..LAYER_COUNT {
        let body = if layer + 1 == LAYER_COUNT {
            "{}".to_owned()
        } else {
            let next = layer + 1;
            format!(r#"{{ "extends": ["layer{next}.json"] }}"#)
        };
        fs::write(project.path().join(format!("layer{layer}.json")), body).expect("write layer");
    }
    if let Err(error) = Project::load(&project.path().join("layer0.json")) {
        assert!(
            error.diags.iter().all(|diag| diag.message.contains("extends")),
            "a chain that is too long must be reported as an `extends` error: {error:?}"
        );
    }
}

#[test]
fn find_config_walks_above_a_relative_start_directory() {
    let project = project_with(&[("luck.json", r#"{ "lua": "lua53" }"#), ("sub/keep.lua", "")]);
    let found = in_directory(&project.path().join("sub"), || find_config(Path::new(".")))
        .expect("discovery from `.` must find the `luck.json` in its parent");
    assert_eq!(
        FileSystemOs.canonicalize_existing(&found),
        FileSystemOs.canonicalize_existing(&project.path().join("luck.json"))
    );
}
