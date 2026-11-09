use luck_linter::{LintConfig, apply_fixes, lint};
use luck_token::{LuaVersion, SourceError};

use crate::common::enable;

#[test]
fn unused_variable_fix_prefixes_underscore() {
    let source = "local unused = 1";
    let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
    let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
    assert_eq!(fixed, "local _unused = 1");
    let reparsed = luck_parser::parse(&fixed, LuaVersion::Lua54);
    assert_eq!(reparsed.errors, Vec::<SourceError>::new());
}

#[test]
fn parenthesized_conditions_fix_removes_parens() {
    let config = enable("parenthesized_conditions");
    let source = "local x = 1\nif (x == 1) then end";
    let diags = lint(source, LuaVersion::Lua54, &config);
    let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
    assert!(fixed.contains("if x == 1 then"));
}

#[test]
fn deprecated_loadstring_fix_switches_to_load() {
    let source = "local f = loadstring('return 1')";
    let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
    let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
    assert!(fixed.contains("load("));
    assert!(!fixed.contains("loadstring"));
}

#[test]
fn no_fix_when_already_underscored() {
    let source = "local _unused = 1";
    let diags = lint(source, LuaVersion::Lua54, &LintConfig::default());
    let fixed = apply_fixes(source, &diags, LuaVersion::Lua54);
    assert_eq!(fixed, source);
}
