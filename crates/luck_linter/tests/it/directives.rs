use luck_linter::{LintConfig, LintDiagnostic, RuleLevel, Severity, lint};
use luck_token::LuaVersion;

use crate::common::with_level;

#[test]
fn allow_covers_next_stmt() {
    let diags = lint(
        "\
-- luck: allow(unused_variable)
local unused = 1",
        LuaVersion::Lua54,
        &LintConfig::default(),
    );
    assert!(diags.iter().all(|diag| diag.rule != "unused_variable"));
}

#[test]
fn trailing_allow_covers_the_stmt_it_trails() {
    let diags = lint(
        "\
local first = 1 -- luck: allow(unused_variable)
local second = 2",
        LuaVersion::Lua54,
        &LintConfig::default(),
    );
    let messages: Vec<&str> = diags
        .iter()
        .filter(|diag| diag.rule == "unused_variable")
        .map(|diag| diag.message.as_str())
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
        LuaVersion::Lua54,
        &LintConfig::default(),
    );
    assert!(diags.iter().all(|diag| diag.rule != "unused_variable"), "{diags:?}");
}

#[test]
fn misspelled_region_word_disables_directive() {
    let diags = lint(
        "\
-- luck: allow(unused_variable) strat
local unused = 1",
        LuaVersion::Lua54,
        &LintConfig::default(),
    );
    assert!(diags.iter().any(|diag| diag.rule == "unused_variable"), "{diags:?}");
}

#[test]
fn deny_escalates_severity() {
    let config = with_level("undefined_variable", RuleLevel::Warn);
    let diags = lint(
        "\
-- luck: deny(undefined_variable)
print(undef)",
        LuaVersion::Lua54,
        &config,
    );
    let denied = diags
        .iter()
        .find(|diag| diag.rule == "undefined_variable")
        .expect("undefined_variable fires");
    assert_eq!(denied.severity, Severity::Error);
}

#[test]
fn warn_downgrades_severity() {
    let config = with_level("undefined_variable", RuleLevel::Error);
    let diags = lint(
        "\
-- luck: warn(undefined_variable)
print(undef)",
        LuaVersion::Lua54,
        &config,
    );
    let downgraded = diags
        .iter()
        .find(|diag| diag.rule == "undefined_variable")
        .expect("undefined_variable fires");
    assert_eq!(downgraded.severity, Severity::Warning);
}

#[test]
fn file_level_allow_covers_whole_file() {
    let source = "\
-- #luck: allow(unused_variable)
local a = 1
local b = 2";
    let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.rule != "unused_variable"));
}

#[test]
fn deny_region_covers_block() {
    let config = with_level("unused_variable", RuleLevel::Warn);
    let source = "\
-- luck: deny(unused_variable) start
local a = 1
local b = 2
-- luck: deny(unused_variable) end";
    let diags = lint(source, LuaVersion::Lua54, &config);
    let region_diags: Vec<_> = diags.iter().filter(|diag| diag.rule == "unused_variable").collect();
    assert_ne!(region_diags, Vec::<&LintDiagnostic>::new());
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
    let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
    let unused_diags: Vec<_> = diags.iter().filter(|diag| diag.rule == "unused_variable").collect();
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
    let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.rule != "unused_variable"));
}

#[test]
fn allow_region_ends_at_its_end_directive() {
    let source = "\
-- luck: allow(unused_variable) start
local a = 1
local b = 2
-- luck: allow(unused_variable) end
local c = 3";
    let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
    assert!(
        diags.iter().all(|diag| !(diag.rule == "unused_variable" && diag.message.contains("`a`")))
    );
    assert!(
        diags.iter().any(|diag| diag.rule == "unused_variable" && diag.message.contains("`c`"))
    );
}

#[test]
fn invalid_lint_filter_fires_on_unknown_name() {
    let source = "\
-- luck: allow(unsed_variable)
local _x = 1";
    let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
    let invalid = diags
        .iter()
        .find(|diag| diag.rule == "invalid_lint_filter")
        .unwrap_or_else(|| panic!("diags: {diags:?}"));
    assert!(invalid.message.contains("unused_variable"));
}

#[test]
fn invalid_lint_filter_silent_on_known() {
    let source = "\
-- luck: allow(unused_variable)
local _x = 1";
    let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
    assert!(diags.iter().all(|diag| diag.rule != "invalid_lint_filter"));
}

#[test]
fn wildcard_allow_covers_only_next_stmt() {
    let source = "\
-- luck: allow(*)
local unused = 1
my_global = 1";
    let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
    assert!(
        diags
            .iter()
            .all(|diag| !(diag.rule == "unused_variable" && diag.message.contains("unused")))
    );
    assert!(diags.iter().any(|diag| diag.rule == "setting_global"));
}

#[test]
fn allow_in_leveled_block_comment_covers_next_stmt() {
    let diags = lint(
        "\
--[=[ luck: allow(unused_variable) ]=]
local unused = 1",
        LuaVersion::Lua54,
        &LintConfig::default(),
    );
    assert!(
        diags.iter().all(|diag| diag.rule != "unused_variable"),
        "a leveled block comment should work like `--[[`: {diags:?}"
    );
}

#[test]
fn allow_after_cr_only_line_break_covers_next_stmt() {
    let diags = lint(
        "local first = 1\r-- luck: allow(unused_variable)\rlocal second = 2",
        LuaVersion::Lua54,
        &LintConfig::default(),
    );
    let messages: Vec<&str> = diags
        .iter()
        .filter(|diag| diag.rule == "unused_variable")
        .map(|diag| diag.message.as_str())
        .collect();
    assert_eq!(
        messages,
        ["unused variable `first`"],
        "the directive is on its own line, so it should cover `second`: {diags:?}"
    );
}
