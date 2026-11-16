//! Each snapshot shows a fixture's every layout once, under the widths that
//! give it, so a change at any width is a diff to review.

use core::fmt::Write as _;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use luck_formatter::FormatOptions;
use luck_testgen::{collect_fixture_files, detect_version, is_error_fixture};

use crate::common::formatted_with;

const LINE_WIDTHS: [u16; 5] = [1, 40, 80, 100, 120];

#[test]
fn every_fixture_formats_as_its_snapshot() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let fixtures = collect_fixture_files(&root).expect("fixture tree is readable");
    let sources: Vec<_> = fixtures
        .iter()
        .filter(|path| matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau")))
        .map(|path| (path, path.strip_prefix(&root).expect("fixture is under the root")))
        .filter(|(_, relative)| !is_error_fixture(relative))
        .collect();
    assert!(!sources.is_empty(), "no fixtures under {}", root.display());

    for (path, relative) in sources {
        let version = detect_version(relative);
        let source = fs::read_to_string(path).expect("fixture is readable");
        let mut layouts: Vec<(Vec<String>, String)> = Vec::new();
        for line_width in LINE_WIDTHS {
            let options = FormatOptions { line_width, ..FormatOptions::default() };
            let output = formatted_with(&source, version, &options);
            if let Some((widths, _)) = layouts.iter_mut().find(|(_, layout)| *layout == output) {
                widths.push(line_width.to_string());
            } else {
                layouts.push((vec![line_width.to_string()], output));
            }
        }
        let mut snapshot = String::new();
        for (widths, layout) in &layouts {
            if !snapshot.is_empty() {
                snapshot.push('\n');
            }
            writeln!(snapshot, "-- line_width = {}", widths.join(", ")).unwrap();
            snapshot.push_str(layout);
        }
        insta::with_settings!({ input_file => path }, {
            insta::assert_snapshot!(relative.display().to_string(), snapshot);
        });
    }
}
