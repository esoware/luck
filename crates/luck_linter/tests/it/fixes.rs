use luck_allocator::Allocator;
use luck_linter::{FixKind, LintConfig};
use luck_token::LuaTarget;

use crate::common::{enable, lint};

/// `source` once every fix of the kinds `fixes` holds is applied, asserting
/// that the fixes converge and that the result parses.
fn fixed(source: &str, config: &LintConfig, fixes: FixKind) -> String {
    let allocator = Allocator::new();
    let result = luck_linter::fix(&allocator, source.as_bytes(), LuaTarget::Lua54, config, fixes)
        .expect("the fixes converge");
    assert!(result.parse.errors.is_empty(), "{:?}", result.parse.errors);
    String::from_utf8(result.parse.source.to_owned()).unwrap()
}

#[test]
fn lint_builds_only_the_fixes_of_the_kinds_asked_for() {
    let allocator = Allocator::new();
    let source = "\
local unused = nil
return 1";
    let config = enable("redundant_nil_init");
    let fix_kinds = |fixes: FixKind| -> Vec<(&str, Option<FixKind>)> {
        luck_linter::lint(&allocator, source.as_bytes(), LuaTarget::Lua54, &config, fixes)
            .iter()
            .map(|diag| (diag.code, diag.fix.as_ref().map(|built| built.kind)))
            .collect()
    };
    assert_eq!(
        fix_kinds(FixKind::empty()),
        [("redundant_nil_init", None), ("unused_variable", None)]
    );
    assert_eq!(
        fix_kinds(FixKind::Fix),
        [("redundant_nil_init", Some(FixKind::Fix)), ("unused_variable", None)]
    );
    assert_eq!(
        fix_kinds(FixKind::Suggestion),
        [("redundant_nil_init", None), ("unused_variable", Some(FixKind::Suggestion))]
    );
}

#[test]
fn fix_returns_the_last_round_s_diagnostics() {
    let allocator = Allocator::new();
    let source = "\
local unused = nil
return 1";
    let config = enable("redundant_nil_init");
    let result =
        luck_linter::fix(&allocator, source.as_bytes(), LuaTarget::Lua54, &config, FixKind::Fix)
            .expect("the fixes converge");
    assert_eq!(
        result.parse.source,
        b"\
local unused
return 1"
    );
    let fixed_source = str::from_utf8(result.parse.source).unwrap();
    assert_eq!(result.messages, lint(fixed_source, LuaTarget::Lua54, &config));
}

/// A fix that copies code keeps the bytes of a string that isn't UTF-8, as
/// a Latin-1 source holds.
#[test]
fn a_fix_copies_bytes_that_are_not_utf8_as_written() {
    let allocator = Allocator::new();
    let source = b"local a = 'caf\xE9'\nlocal b = '\xFF' -- d\xE9j\xE0\nreturn a, b";
    let config = enable("merge_adjacent_locals");
    let result = luck_linter::fix(&allocator, source, LuaTarget::Lua51, &config, FixKind::Fix)
        .expect("the fixes converge");
    assert_eq!(result.parse.source, b"local a, b = 'caf\xE9', '\xFF' -- d\xE9j\xE0\nreturn a, b");
}

#[test]
fn fix_returns_the_source_when_no_rule_fires() {
    let source = "\
local x = 1
return x";
    assert_eq!(fixed(source, &LintConfig::default(), FixKind::all()), source);
}

#[test]
fn fix_leaves_the_underscore_suggestion_alone() {
    let source = "local unused = 1";
    assert_eq!(fixed(source, &LintConfig::default(), FixKind::Fix), source);
}

#[test]
fn fix_suggestions_prefix_an_unused_variable() {
    assert_eq!(
        fixed("local unused = 1", &LintConfig::default(), FixKind::all()),
        "local _unused = 1"
    );
}

#[test]
fn parenthesized_conditions_fix_removes_parens() {
    let source = "\
local x = 1
if (x == 1) then end";
    let fixed = fixed(source, &enable("parenthesized_conditions"), FixKind::Fix);
    assert!(fixed.contains("if x == 1 then"), "{fixed}");
}

#[test]
fn deprecated_loadstring_fix_switches_to_load() {
    let fixed = fixed("local f = loadstring('return 1')", &LintConfig::default(), FixKind::Fix);
    assert_eq!(fixed, "local f = load('return 1')");
}

#[test]
fn deprecated_getn_is_only_suggested_since_length_calls_len() {
    let source = "return table.getn(t)";
    assert_eq!(fixed(source, &LintConfig::default(), FixKind::Fix), source);
    assert_eq!(fixed(source, &LintConfig::default(), FixKind::all()), "return #t");
}

#[test]
fn no_fix_when_already_underscored() {
    let source = "local _unused = 1";
    assert_eq!(fixed(source, &LintConfig::default(), FixKind::all()), source);
}

#[test]
fn no_fix_deletes_a_comment() {
    let source = "\
local x = --[[why]] nil
return x";
    assert_eq!(fixed(source, &enable("redundant_nil_init"), FixKind::all()), source);
}

#[test]
fn fixes_overlapping_ones_in_later_rounds() {
    let source = "return not (a == (not (b == c)))";
    assert_eq!(
        fixed(source, &enable("unnecessary_negation"), FixKind::Fix),
        "return a ~= (b ~= c)"
    );
}
