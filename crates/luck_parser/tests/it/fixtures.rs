use std::fmt::Write as _;

use luck_allocator::Allocator;
use luck_parser::{parse, parse_with_tokens};
use luck_testgen::lua_fixtures;

#[test]
fn fixtures_outside_errors_parse_cleanly() {
    let allocator = Allocator::new();
    let mut failures = Vec::new();
    let mut success_count = 0;

    for fixture in lua_fixtures() {
        let result = parse(&allocator, fixture.source.as_bytes(), fixture.version);

        if fixture.is_error {
            // Fixtures under `errors/` may or may not parse.
            continue;
        }

        if result.errors.is_empty() {
            success_count += 1;
        } else {
            failures.push((fixture.path, result.errors));
        }
    }

    if !failures.is_empty() {
        let mut report = format!("{} fixture(s) failed to parse:\n", failures.len());
        for (path, errors) in &failures {
            writeln!(report, "  {}:", path.display()).unwrap();
            for e in errors {
                writeln!(report, "    - {}", e.message).unwrap();
            }
        }
        panic!("{report}{success_count} fixture(s) parsed successfully");
    }

    assert!(success_count > 0, "parsed {success_count} fixtures successfully");
}

#[test]
fn every_fixture_parses_with_the_tokens_it_lexes_to() {
    for fixture in lua_fixtures() {
        let (source, version) = (fixture.source.as_bytes(), fixture.version);
        let allocator = Allocator::new();
        let tokens = parse_with_tokens(&allocator, source, version).tokens;
        assert_eq!(
            tokens,
            luck_lexer::lex(source, version).tokens,
            "{}",
            fixture.relative_path.display()
        );
    }
}
