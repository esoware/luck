use core::time::Duration;
use std::fs;
use std::path::Path;

use luck_config::{Project, find_config};
use tempfile::TempDir;

use crate::common::{finishes_within, in_directory, project_with};

#[test]
fn root_config_rejects_an_escape_through_a_transitive_extends() {
    let project = project_with(&[
        ("outside.json", r#"{ "lua": "lua51" }"#),
        ("project/base.json", r#"{ "extends": ["../outside.json"] }"#),
        ("project/luck.json", r#"{ "root": true, "extends": ["base.json"] }"#),
    ]);
    let diag = Project::load(&project.path().join("project/luck.json"))
        .expect_err("a chain leaving a `root` config's directory must be rejected");
    assert!(diag.file_path.ends_with("base.json"), "{diag:?}");
    assert!(diag.message.contains("outside"), "{diag:?}");
}

#[test]
fn parse_error_in_an_extended_file_names_that_file() {
    let project = project_with(&[
        ("base.json", "{ broken"),
        ("luck.json", r#"{ "extends": ["base.json"] }"#),
    ]);
    let diag = Project::load(&project.path().join("luck.json"))
        .expect_err("a malformed parent must fail to load");
    assert!(
        diag.file_path.ends_with("base.json"),
        "the error must name the file that failed to parse, not `luck.json`: {diag:?}"
    );
}

#[test]
fn a_config_that_cannot_be_read_is_e010() {
    let project = project_with(&[]);
    let diag = Project::load(&project.path().join("luck.json")).unwrap_err();
    assert_eq!(diag.code, "E010");
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
    if let Err(diag) = Project::load(&project.path().join("layer0.json")) {
        assert!(
            diag.message.contains("extends"),
            "a chain that is too long must be reported as an `extends` error: {diag:?}"
        );
    }
}

#[test]
fn find_config_walks_above_a_relative_start_directory() {
    let project = project_with(&[("luck.json", r#"{ "lua": "lua53" }"#), ("sub/keep.lua", "")]);
    let found = in_directory(&project.path().join("sub"), || find_config(Path::new(".")))
        .expect("discovery from `.` must find the `luck.json` in its parent");
    assert_eq!(
        found.canonicalize().expect("exists"),
        project.path().join("luck.json").canonicalize().expect("exists")
    );
}
