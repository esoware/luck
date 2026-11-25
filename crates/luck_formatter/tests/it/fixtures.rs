//! Each snapshot shows a fixture's every layout once, under the widths that
//! give it, so a change at any width is a diff to review.

use std::fmt::Write as _;

use luck_formatter::FormatOptions;
use luck_testgen::luau_fixtures;

use crate::common::formatted_with;

const LINE_WIDTHS: [u16; 5] = [1, 40, 80, 100, 120];

#[test]
fn every_fixture_formats_as_its_snapshot() {
    let fixtures: Vec<_> =
        luau_fixtures().into_iter().filter(|fixture| !fixture.is_error).collect();
    assert!(!fixtures.is_empty(), "no fixture outside `errors/`");

    for fixture in fixtures {
        let mut layouts: Vec<(Vec<String>, String)> = Vec::new();
        for line_width in LINE_WIDTHS {
            let options = FormatOptions { line_width, ..FormatOptions::default() };
            let formatted = formatted_with(fixture.source.as_bytes(), &options);
            let output = String::from_utf8(formatted).expect("a fixture formats as UTF-8");
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
        insta::with_settings!({ input_file => &fixture.path }, {
            insta::assert_snapshot!(fixture.relative_path.display().to_string(), snapshot);
        });
    }
}
