use crate::common::{check, checker, described};

const MISMATCH: &str = "\
--!strict
local count: number = \"one\"
return count
";

#[test]
fn a_type_error_is_an_e015_in_luaus_words_at_its_span() {
    let mut checker = checker(&[("main", MISMATCH)]);

    let diags = check(&mut checker, &["main"]).unwrap();

    assert_eq!(
        described(&diags[0], MISMATCH),
        [("E015", "Expected this to be 'number', but got 'string'".to_owned(), "\"one\"")]
    );
}

#[test]
fn a_clean_module_has_no_diagnostics() {
    let source = "\
--!strict
local count: number = 1
return count + 1
";
    let mut checker = checker(&[("main", source)]);

    assert_eq!(check(&mut checker, &["main"]).unwrap(), [[]]);
}

const POINT_MODULE: &str = "\
--!strict
export type Point = { x: number }
local origin: Point = { x = 0 }
return { origin = origin }
";

#[test]
fn a_required_modules_exported_type_flows_into_the_requirer() {
    let main = "\
--!strict
local point = require(\"./point\")
local origin: point.Point = point.origin
local bad: point.Point = { x = \"zero\" }
return origin, bad
";
    let mut checker = checker(&[("main", main), ("point", POINT_MODULE)]);

    let diags = check(&mut checker, &["main", "point"]).unwrap();

    let main_diags = described(&diags[0], main);
    assert_eq!(main_diags.len(), 1, "{main_diags:?}");
    assert_eq!(main_diags[0].2, "\"zero\"", "{main_diags:?}");
    assert!(diags[1].is_empty(), "{:?}", diags[1]);
}

#[test]
fn an_unresolved_require_is_unknown_in_strict_mode_only() {
    let strict = "\
--!strict
local missing = require(\"./missing\")
return missing
";
    let nonstrict = "\
local missing = require(\"./missing\")
return missing
";
    let mut checker = checker(&[("strict", strict), ("nonstrict", nonstrict)]);

    let diags = check(&mut checker, &["strict", "nonstrict"]).unwrap();

    assert_eq!(
        described(&diags[0], strict),
        [("E015", "Unknown require: unsupported path".to_owned(), "require(\"./missing\")")]
    );
    assert!(diags[1].is_empty(), "{:?}", diags[1]);
}

#[test]
fn a_syntax_error_is_an_e008() {
    let source = "local x = \n";
    let mut checker = checker(&[("main", source)]);

    let diags = check(&mut checker, &["main"]).unwrap();

    assert_eq!(diags[0].len(), 1, "{:?}", diags[0]);
    assert_eq!(diags[0][0].code, "E008");
}

#[test]
fn a_module_with_no_source_has_no_diagnostics() {
    let mut checker = checker(&[]);

    assert_eq!(check(&mut checker, &["gone"]).unwrap(), [[]]);
}

#[test]
fn a_luaurc_sets_the_mode_a_hot_comment_overrides() {
    let unannotated = "local count: number = \"one\"\nreturn count\n";
    let unchecked = "--!nocheck\nlocal count: number = \"one\"\nreturn count\n";
    let mut checker = checker(&[("unannotated", unannotated), ("unchecked", unchecked)]);
    // The alias comes first, which Luau's parser stops at without alias
    // options.
    checker.host_mut().luaurc =
        Some(r#"{ "aliases": { "lib": "./lib" }, "languageMode": "strict" }"#.to_owned());

    let diags = check(&mut checker, &["unannotated", "unchecked"]).unwrap();

    assert_eq!(diags[0].len(), 1, "{:?}", diags[0]);
    assert!(diags[1].is_empty(), "{:?}", diags[1]);
}

#[test]
fn many_modules_check_across_threads() {
    let modules: Vec<(String, String)> = (0..32)
        .map(|index| {
            (format!("m{index}"), format!("--!strict\nlocal x: string = {index}\nreturn x\n"))
        })
        .collect();
    let sources: Vec<(&str, &str)> =
        modules.iter().map(|(name, source)| (name.as_str(), source.as_str())).collect();
    let names: Vec<&str> = sources.iter().map(|&(name, _)| name).collect();
    let mut checker = checker(&sources);

    let diags = check(&mut checker, &names).unwrap();

    assert!(diags.iter().all(|module| module.len() == 1 && module[0].code == "E015"), "{diags:?}");
}

#[test]
fn a_luaurc_luau_rejects_returns_luaus_error_and_keeps_the_keys_before() {
    let mut checker = checker(&[(
        "main",
        "local count: number = \"one\"
return count
",
    )]);
    checker.host_mut().luaurc = Some("{ \"languageMode\": \"strict\" \"lint\" }".to_owned());

    let diags = check(&mut checker, &["main"]).unwrap();

    assert_eq!(diags[0].len(), 1, "{:?}", diags[0]);
    let errors = checker.host_mut().luaurc_errors.get_mut();
    assert!(!errors.is_empty(), "{errors:?}");
    assert_eq!(errors[0], "Expected ',' or '}' at line 1, got \"lint\" instead");
}
