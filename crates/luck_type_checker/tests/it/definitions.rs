use luck_allocator::Allocator;
use luck_type_checker::builtin_definitions;

use crate::common::{check, checker, described};

#[test]
fn definitions_loaded_from_text_declare_globals() {
    let main = "\
--!strict
local greeting: number = greet(\"world\")
return greeting
";
    let mut checker = checker(&[("main", main)]);

    checker.load_definitions("@app", "declare function greet(name: string): string\n").unwrap();
    let diags = check(&mut checker, &["main"]).unwrap();

    assert_eq!(
        described(&diags[0], main),
        [("E015", "Expected this to be 'number', but got 'string'".to_owned(), "greet(\"world\")")]
    );
}

#[test]
fn definitions_that_dont_parse_report_e008_in_their_text() {
    let mut checker = checker(&[]);
    let source = "declare function greet(name: string: string\n";

    let diags = checker.load_definitions("@app", source).unwrap_err();

    assert_eq!(diags[0].code, "E008", "{diags:?}");
    assert!(diags[0].span.is_some_and(|span| span.end as usize <= source.len()), "{diags:?}");
}

#[test]
fn definitions_with_a_type_error_report_e015_and_load_nothing() {
    let main = "\
--!strict
return greet
";
    let mut checker = checker(&[("main", main)]);
    let source = "declare function greet(name: Missing): string\n";

    let diags = checker.load_definitions("@app", source).unwrap_err();
    let after = check(&mut checker, &["main"]).unwrap();

    assert_eq!(described(&diags, source)[0].0, "E015", "{diags:?}");
    assert_eq!(described(&diags, source)[0].2, "Missing", "{diags:?}");
    assert_eq!(after[0].len(), 1, "{after:?}");
}

#[test]
fn the_builtin_definitions_are_luaus_and_luck_parses_them() {
    let source = builtin_definitions();

    assert!(source.contains("declare function print"), "{source}");
    let allocator = Allocator::new();
    let parsed = luck_parser::parse_definitions(&allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
}
