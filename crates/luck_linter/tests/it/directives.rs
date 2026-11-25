use luck_allocator::Allocator;
use luck_diagnostics::Severity;
use luck_linter::{FixKind, LintConfig, Linter, Message, RuleId, RuleLevel};
use luck_stdlib::library_for;
use luck_token::Variant;

use crate::common::{lint, lint_bytes, with_level};

#[test]
fn allow_covers_next_stmt() {
    let diags = lint(
        "\
-- luck: allow(unused_variable)
local unused = 1",
        &LintConfig::default(),
    );
    assert!(diags.iter().all(|diag| diag.code != "unused_variable"));
}

#[test]
fn trailing_allow_covers_the_stmt_it_trails() {
    let diags = lint(
        "\
local first = 1 -- luck: allow(unused_variable)
local second = 2",
        &LintConfig::default(),
    );
    let messages: Vec<&str> = diags
        .iter()
        .filter(|diag| diag.code == "unused_variable")
        .map(|diag| &*diag.message)
        .collect();
    assert_eq!(messages.len(), 1, "{diags:?}");
    assert!(messages[0].contains("`second`"), "{diags:?}");
}

#[test]
fn block_comment_allow_covers_next_stmt() {
    let diags = lint(
        "\
--[[ luck: allow(unused_variable) ]]
local unused = 1",
        &LintConfig::default(),
    );
    assert!(diags.iter().all(|diag| diag.code != "unused_variable"), "{diags:?}");
}

#[test]
fn misspelled_region_word_disables_directive() {
    let diags = lint(
        "\
-- luck: allow(unused_variable) strat
local unused = 1",
        &LintConfig::default(),
    );
    assert!(diags.iter().any(|diag| diag.code == "unused_variable"), "{diags:?}");
}

#[test]
fn deny_escalates_severity() {
    let config = with_level(RuleId::UndefinedVariable, RuleLevel::Warn);
    let diags = lint(
        "\
-- luck: deny(undefined_variable)
print(undef)",
        &config,
    );
    let denied = diags
        .iter()
        .find(|diag| diag.code == "undefined_variable")
        .expect("undefined_variable fires");
    assert_eq!(denied.severity, Severity::Error);
}

#[test]
fn warn_downgrades_severity() {
    let config = with_level(RuleId::UndefinedVariable, RuleLevel::Error);
    let diags = lint(
        "\
-- luck: warn(undefined_variable)
print(undef)",
        &config,
    );
    let downgraded = diags
        .iter()
        .find(|diag| diag.code == "undefined_variable")
        .expect("undefined_variable fires");
    assert_eq!(downgraded.severity, Severity::Warning);
}

#[test]
fn file_level_allow_covers_whole_file() {
    let source = "\
-- #luck: allow(unused_variable)
local a = 1
local b = 2";
    let diags = lint(source, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.code != "unused_variable"));
}

#[test]
fn deny_region_covers_block() {
    let config = with_level(RuleId::UnusedVariable, RuleLevel::Warn);
    let source = "\
-- luck: deny(unused_variable) start
local a = 1
local b = 2
-- luck: deny(unused_variable) end";
    let diags = lint(source, &config);
    let region_diags: Vec<_> = diags.iter().filter(|diag| diag.code == "unused_variable").collect();
    assert_ne!(region_diags, Vec::<&Message>::new());
    for diag in region_diags {
        assert_eq!(diag.severity, Severity::Error);
    }
}

#[test]
fn stmt_allow_wins_over_enclosing_deny_region() {
    let source = "\
-- luck: deny(unused_variable) start
-- luck: allow(unused_variable)
local a = 1
local b = 2
-- luck: deny(unused_variable) end";
    let diags = lint(source, &LintConfig::default());
    let unused_diags: Vec<_> = diags.iter().filter(|diag| diag.code == "unused_variable").collect();
    assert_eq!(unused_diags.len(), 1, "{diags:?}");
    assert!(unused_diags[0].message.contains("`b`"), "{diags:?}");
    assert_eq!(unused_diags[0].severity, Severity::Error);
}

#[test]
fn unclosed_region_extends_to_end_of_file() {
    let source = "\
-- luck: allow(unused_variable) start
local a = 1
local b = 2";
    let diags = lint(source, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.code != "unused_variable"));
}

#[test]
fn allow_region_ends_at_its_end_directive() {
    let source = "\
-- luck: allow(unused_variable) start
local a = 1
local b = 2
-- luck: allow(unused_variable) end
local c = 3";
    let diags = lint(source, &LintConfig::default());
    assert!(
        diags.iter().all(|diag| !(diag.code == "unused_variable" && diag.message.contains("`a`")))
    );
    assert!(
        diags.iter().any(|diag| diag.code == "unused_variable" && diag.message.contains("`c`"))
    );
}

#[test]
fn invalid_lint_filter_fires_on_unknown_name() {
    let source = "\
-- luck: allow(unsed_variable)
local _x = 1";
    let diags = lint(source, &LintConfig::default());
    let invalid = diags
        .iter()
        .find(|diag| diag.code == "invalid_lint_filter")
        .unwrap_or_else(|| panic!("diags: {diags:?}"));
    assert!(invalid.message.contains("unused_variable"));
}

#[test]
fn invalid_lint_filter_silent_on_known() {
    let source = "\
-- luck: allow(unused_variable)
local _x = 1";
    let diags = lint(source, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.code != "invalid_lint_filter"));
}

#[test]
fn invalid_lint_filter_shows_a_word_that_is_not_utf8() {
    let source = b"-- luck: allow(x) \xE9t\xE9\nlocal _x = 1";
    let diags = lint_bytes(source, Variant::Standalone, &LintConfig::default(), FixKind::empty());
    let found: Vec<(&str, &[u8])> = diags
        .iter()
        .filter(|diag| diag.code == "invalid_lint_filter")
        .map(|diag| (&*diag.message, diag.span().text(source)))
        .collect();
    assert_eq!(
        found,
        [(
            "expected `start` or `end` after `allow(...)`, found `\u{FFFD}t\u{FFFD}`",
            &b"\xE9t\xE9"[..]
        )]
    );
}

#[test]
fn wildcard_allow_covers_only_next_stmt() {
    let source = "\
-- luck: allow(*)
local unused = 1
my_global = 1";
    let diags = lint(source, &LintConfig::default());
    assert!(
        diags
            .iter()
            .all(|diag| !(diag.code == "unused_variable" && diag.message.contains("unused")))
    );
    assert!(diags.iter().any(|diag| diag.code == "setting_global"));
}

#[test]
fn allow_in_leveled_block_comment_covers_next_stmt() {
    let diags = lint(
        "\
--[=[ luck: allow(unused_variable) ]=]
local unused = 1",
        &LintConfig::default(),
    );
    assert!(
        diags.iter().all(|diag| diag.code != "unused_variable"),
        "a leveled block comment should work like `--[[`: {diags:?}"
    );
}

#[test]
fn deny_runs_a_rule_config_turns_off_only_where_it_applies() {
    let source = "\
local function f()
  -- luck: deny(shadowing)
  local f = 1
  return f
end
local function g()
  local g = 1
  return g
end
return f, g";
    let diags = lint(source, &LintConfig::default());
    let shadowing: Vec<(&str, Severity)> = diags
        .iter()
        .filter(|diag| diag.code == "shadowing")
        .map(|diag| (&*diag.message, diag.severity))
        .collect();
    assert_eq!(shadowing.len(), 1, "{diags:?}");
    assert!(shadowing[0].0.contains("`f`"), "{diags:?}");
    assert_eq!(shadowing[0].1, Severity::Error);
}

#[test]
fn warn_in_a_file_level_directive_runs_its_rule_everywhere() {
    let source = "\
-- #luck: warn(shadowing)
local a = 1
do
  local a = 2
  print(a)
end
print(a)";
    let diags = lint(source, &LintConfig::default());
    assert!(
        diags.iter().any(|diag| diag.code == "shadowing" && diag.severity == Severity::Warning),
        "{diags:?}"
    );
}

#[test]
fn allow_inside_a_statement_with_none_after_it_covers_that_statement() {
    let source = "\
local t = {
  a = 1,
  a = 2, -- luck: allow(duplicate_keys)
}
return t";
    let diags = lint(source, &LintConfig::default());
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn trailing_allow_after_a_separator_covers_the_stmt_before_it() {
    let diags = lint("local unused = 1; -- luck: allow(unused_variable)", &LintConfig::default());
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn formatter_directive_is_neither_lint_directive_nor_invalid() {
    let diags = lint(
        "\
-- luck: format off
local   _x = 1
-- luck: format on",
        &LintConfig::default(),
    );
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn the_parse_error_code_is_no_rule_a_directive_can_name() {
    let diags = lint(
        "\
-- #luck: allow(E008)
local _x = 1",
        &LintConfig::default(),
    );
    let rules: Vec<&str> = diags.iter().map(|diag| diag.code).collect();
    assert_eq!(rules, ["invalid_lint_filter"], "{diags:?}");
    let broken = lint(
        "\
-- #luck: allow(E008)
local x =",
        &LintConfig::default(),
    );
    let broken_rules: Vec<&str> = broken.iter().map(|diag| diag.code).collect();
    assert_eq!(broken_rules, ["E008"], "{broken:?}");
}

#[test]
fn fix_removes_an_allow_that_silences_nothing() {
    let source = "\
local a = 1
-- luck: allow(undefined_variable)
print(a) -- luck: allow(unused_variable, undefined_variable)
";
    let allocator = Allocator::new();
    let fixed = Linter::new(LintConfig::default())
        .fix(&allocator, source.as_bytes(), library_for(Variant::Standalone), FixKind::Fix, None)
        .expect("fixes converge");
    assert_eq!(
        fixed.parse.source,
        b"\
local a = 1
print(a)
"
    );
    assert_eq!(fixed.messages, []);
}

#[test]
fn unused_allow_report_is_itself_a_rule_a_directive_can_allow() {
    let source = "\
-- #luck: allow(unused_lint_filter)
-- luck: allow(undefined_variable)
local a = 1
print(a)";
    assert_eq!(lint(source, &LintConfig::default()), []);
    let mut config = LintConfig::default();
    config.rules.set_level(RuleId::UnusedLintFilter, RuleLevel::Off);
    assert_eq!(
        lint(
            "\
-- luck: allow(undefined_variable)
local a = 1
print(a)",
            &config
        ),
        []
    );
}
