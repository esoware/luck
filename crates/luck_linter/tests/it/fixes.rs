use luck_allocator::Allocator;
use luck_linter::{FixKind, LintConfig, Linter, RuleId};
use luck_stdlib::{SecurityLevel, luau_library, roblox_library};

use crate::common::{enable, lint, lint_bytes};

/// `source` once every fix of the kinds `fixes` holds is applied, asserting
/// that the fixes converge and that the result parses.
fn fixed(source: &str, config: &LintConfig, fixes: FixKind) -> String {
    let allocator = Allocator::new();
    let parse = luck_parser::parse(&allocator, source.as_bytes());
    let result = Linter::new(config.clone())
        .fix(&allocator, parse, luau_library(), fixes, None)
        .expect("the fixes converge");
    assert!(result.parse.errors.is_empty(), "{:?}", result.parse.errors);
    String::from_utf8(result.parse.source.to_owned()).unwrap()
}

#[test]
fn lint_builds_only_the_fixes_of_the_kinds_asked_for() {
    let source = "\
local unused = nil
return 1";
    let config = enable(RuleId::RedundantNilInit);
    let fix_kinds = |fixes: FixKind| -> Vec<(&str, Option<FixKind>)> {
        lint_bytes(source.as_bytes(), luau_library(), &config, fixes)
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
    let config = enable(RuleId::RedundantNilInit);
    let parse = luck_parser::parse(&allocator, source.as_bytes());
    let result = Linter::new(config.clone())
        .fix(&allocator, parse, luau_library(), FixKind::Fix, None)
        .expect("the fixes converge");
    assert_eq!(
        result.parse.source,
        b"\
local unused
return 1"
    );
    let fixed_source = str::from_utf8(result.parse.source).unwrap();
    assert_eq!(result.messages, lint(fixed_source, &config));
}

/// A fix that copies code keeps the bytes of a string that isn't UTF-8, as
/// a Latin-1 source holds.
#[test]
fn a_fix_copies_bytes_that_are_not_utf8_as_written() {
    let allocator = Allocator::new();
    let source = b"local a = 'caf\xE9'\nlocal b = '\xFF' -- d\xE9j\xE0\nreturn a, b";
    let parse = luck_parser::parse(&allocator, source);
    let result = Linter::new(enable(RuleId::MergeAdjacentLocals))
        .fix(&allocator, parse, luau_library(), FixKind::Fix, None)
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
    let fixed = fixed(source, &enable(RuleId::ParenthesizedConditions), FixKind::Fix);
    assert!(fixed.contains("if x == 1 then"), "{fixed}");
}

/// A definition can't say a replacement behaves the same, so what its `use`
/// names is only a suggestion.
#[test]
fn a_deprecation_s_replacement_is_only_suggested() {
    let fixed_on_roblox = |fixes: FixKind| {
        let allocator = Allocator::new();
        let parse = luck_parser::parse(&allocator, b"wait(1)");
        let result = Linter::new(LintConfig::default())
            .fix(&allocator, parse, roblox_library(SecurityLevel::None), fixes, None)
            .expect("the fixes converge");
        String::from_utf8(result.parse.source.to_owned()).unwrap()
    };
    assert_eq!(fixed_on_roblox(FixKind::Fix), "wait(1)");
    assert_eq!(fixed_on_roblox(FixKind::all()), "task.wait(1)");
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
    assert_eq!(fixed(source, &enable(RuleId::RedundantNilInit), FixKind::all()), source);
}

#[test]
fn fixes_overlapping_ones_in_later_rounds() {
    let source = "return not (a == (not (b == c)))";
    assert_eq!(
        fixed(source, &enable(RuleId::UnnecessaryNegation), FixKind::Fix),
        "return a ~= (b ~= c)"
    );
}

/// A definition file holds no code for a rule to read, so even its ordinary
/// statements find nothing, and there's nothing in it to fix.
#[test]
fn a_definition_file_yields_its_syntax_errors_alone() {
    let allocator = Allocator::new();
    let linter = Linter::new(LintConfig::default());
    let library = luau_library();
    let source = b"\
local unused = nil
print(undefined_name)";
    let code = luck_parser::parse(&allocator, source);
    assert!(!linter.run(&code, library, FixKind::all(), None).is_empty(), "code lints");
    let definitions = luck_parser::parse_definitions(&allocator, source);
    assert_eq!(linter.run(&definitions, library, FixKind::all(), None), []);
    let analysis = linter.analyze(&definitions, library);
    assert_eq!(linter.run_analyzed(&analysis, FixKind::all(), None, &|| false), Some(Vec::new()));
    let fixed = linter
        .fix(
            &allocator,
            luck_parser::parse_definitions(&allocator, source),
            library,
            FixKind::all(),
            None,
        )
        .expect("nothing to fix");
    assert_eq!((fixed.parse.source, fixed.messages), (&source[..], Vec::new()));
    let broken = luck_parser::parse_definitions(&allocator, b"declare function f(x)");
    let messages = linter.run(&broken, library, FixKind::all(), None);
    let codes: Vec<&str> = messages.iter().map(|message| message.code).collect();
    assert_eq!(codes, ["E008"], "{messages:?}");
}
