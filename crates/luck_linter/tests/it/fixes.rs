use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_linter::{LintConfig, LintDiagnostic, apply_fixes};
use luck_token::LuaVersion;

use crate::common::{enable, lint};

/// What `apply_fixes` leaves of `source`, which is `source` itself when it
/// applies nothing.
fn fixed(source: &str, diags: &[LintDiagnostic]) -> String {
    let allocator = Allocator::new();
    apply_fixes(&allocator, source, diags, LuaVersion::Lua54)
        .map_or_else(|| source.to_owned(), |parse| parse.source.to_owned())
}

#[test]
fn unused_variable_fix_prefixes_underscore() {
    let source = "local unused = 1";
    let diags = lint(source, LuaTarget::Lua54, &LintConfig::default());
    let allocator = Allocator::new();
    let fixed = apply_fixes(&allocator, source, &diags, LuaVersion::Lua54).expect("a fix applies");
    assert_eq!(fixed.source, "local _unused = 1");
    assert!(fixed.errors.is_empty(), "{:?}", fixed.errors);
}

#[test]
fn parenthesized_conditions_fix_removes_parens() {
    let config = enable("parenthesized_conditions");
    let source = "\
local x = 1
if (x == 1) then end";
    let diags = lint(source, LuaTarget::Lua54, &config);
    assert!(fixed(source, &diags).contains("if x == 1 then"));
}

#[test]
fn deprecated_loadstring_fix_switches_to_load() {
    let source = "local f = loadstring('return 1')";
    let diags = lint(source, LuaTarget::Lua54, &LintConfig::default());
    let fixed = fixed(source, &diags);
    assert!(fixed.contains("load("));
    assert!(!fixed.contains("loadstring"));
}

#[test]
fn no_fix_when_already_underscored() {
    let source = "local _unused = 1";
    let diags = lint(source, LuaTarget::Lua54, &LintConfig::default());
    let allocator = Allocator::new();
    assert!(apply_fixes(&allocator, source, &diags, LuaVersion::Lua54).is_none());
}
