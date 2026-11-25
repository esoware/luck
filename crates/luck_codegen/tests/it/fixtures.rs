use std::fmt::Write as _;

use luck_allocator::Allocator;
use luck_codegen::LiteralSpelling;
use luck_testgen::{luau_fixtures, tree_shape};

#[test]
fn every_fixture_compacts_to_output_that_compacts_to_itself() {
    let mut report = String::new();
    let mut round_trip_count = 0;
    for fixture in luau_fixtures() {
        if fixture.is_error {
            continue;
        }
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, fixture.source.as_bytes());
        if !parsed.errors.is_empty() {
            continue;
        }
        let shape = tree_shape(&parsed.chunk);
        for (label, spelling) in
            [("as written", LiteralSpelling::AsWritten), ("shortest", LiteralSpelling::Shortest)]
        {
            let output = luck_codegen::compact(&parsed.chunk, spelling);
            let reparsed = luck_parser::parse(&allocator, &output);
            let text = String::from_utf8_lossy(&output);
            let name = fixture.relative_path.display();
            if let Some(error) = reparsed.errors.first() {
                writeln!(report, "{name} {label}: {text}\n  {}", error.message).unwrap();
            } else if tree_shape(&reparsed.chunk) != shape {
                writeln!(report, "{name} {label}: {text} parses to a different tree").unwrap();
            } else if luck_codegen::compact(&reparsed.chunk, spelling) == output {
                round_trip_count += 1;
            } else {
                writeln!(report, "{name} {label}: compacting again changed {text}").unwrap();
            }
        }
    }
    assert!(report.is_empty(), "{report}{round_trip_count} fixture(s) round-tripped");
    assert!(round_trip_count > 0, "no fixture parsed");
}
