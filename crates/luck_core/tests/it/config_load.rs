use core::time::Duration;
use std::fs;
use std::path::Path;

use luck_core::{discover_config, load_with_extends};
use tempfile::TempDir;

use crate::common::{finishes_within, in_directory, project_with};

#[test]
fn root_config_rejects_an_escape_through_a_transitive_extends() {
    let project = project_with(&[
        ("outside.json", r#"{ "lua": "lua51" }"#),
        ("project/base.json", r#"{ "extends": ["../outside.json"] }"#),
        ("project/luck.json", r#"{ "root": true, "extends": ["base.json"] }"#),
    ]);
    let message = load_with_extends(&project.path().join("project/luck.json"))
        .expect_err("a chain leaving a `root` config's directory must be rejected");
    assert!(message.contains("escapes the root boundary"), "unexpected error: {message}");
}

#[test]
fn parse_error_in_an_extended_file_names_that_file() {
    let project = project_with(&[
        ("base.json", "{ broken"),
        ("luck.json", r#"{ "extends": ["base.json"] }"#),
    ]);
    let message = load_with_extends(&project.path().join("luck.json"))
        .expect_err("a malformed parent must fail to load");
    assert!(
        message.contains("base.json"),
        "the error must name the file that failed to parse, not `luck.json`: {message}"
    );
}

#[test]
fn repeated_extends_layers_resolve_promptly() {
    const LAYER_COUNT: usize = 40;
    let project = TempDir::new().expect("create temp dir");
    for layer in 0..LAYER_COUNT {
        let body = if layer + 1 == LAYER_COUNT {
            "{}".to_owned()
        } else {
            let next = layer + 1;
            format!(r#"{{ "extends": ["layer{next}.json", "layer{next}.json"] }}"#)
        };
        fs::write(project.path().join(format!("layer{layer}.json")), body).expect("write layer");
    }
    let first_layer = project.path().join("layer0.json");
    let is_finished =
        finishes_within(Duration::from_secs(10), move || load_with_extends(&first_layer));
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
    if let Err(message) = load_with_extends(&project.path().join("layer0.json")) {
        assert!(
            message.contains("extends"),
            "a chain that is too long must be reported as an `extends` error: {message}"
        );
    }
}

#[test]
fn discover_config_walks_above_a_relative_start_directory() {
    let project = project_with(&[("luck.json", r#"{ "lua": "lua53" }"#), ("sub/keep.lua", "")]);
    let discovered =
        in_directory(&project.path().join("sub"), || discover_config(Path::new("."))).unwrap();
    assert!(discovered.is_some(), "discovery from `.` must find the `luck.json` in its parent");
}
