//! `blocks_equivalent`, the structural comparison behind verified formatting.

use luck_formatter::{AstDiff, blocks_equivalent};
use luck_token::LuaVersion;

/// Where `left` and `right`, parsed as `version`, first diverge.
fn divergence(version: LuaVersion, left: &str, right: &str) -> Option<AstDiff> {
    let [left_block, right_block] = [left, right].map(|source| {
        let parsed = luck_parser::parse(source, version);
        assert!(parsed.errors.is_empty(), "parse error: {:?}", parsed.errors);
        parsed.block
    });
    blocks_equivalent(&left_block, &right_block, version).err()
}

#[test]
fn equivalent_when_formatting_differs() {
    let diff = divergence(LuaVersion::Lua54, "local x = 1\nlocal y = 2", "local x=1\nlocal y=2");
    assert_eq!(diff, None);
}

#[test]
fn paren_strip_is_equivalent() {
    let diff = divergence(LuaVersion::Lua54, "local x = (1 + 2)", "local x = 1 + 2");
    assert_eq!(diff, None);
}

#[test]
fn call_paren_normalization_is_equivalent() {
    let diff = divergence(LuaVersion::Lua54, "print(\"hi\")", "print\"hi\"");
    assert_eq!(diff, None);
}

#[test]
fn detects_renamed_identifier() {
    // `TokenKind::Identifier` carries the name, so a rename changes the
    // token kind.
    let diff =
        divergence(LuaVersion::Lua54, "local x = 1", "local y = 1").expect("rename should diverge");
    assert!(diff.reason.contains("kind"), "got: {}", diff.reason);
}

#[test]
fn detects_missing_stmt() {
    // The diff has to say where the trees first diverge, not just that they
    // do.
    let diff = divergence(LuaVersion::Lua54, "local x = 1\nlocal y = 2", "local x = 1")
        .expect("should diverge");
    assert!(diff.path.contains("block"), "got path: {}", diff.path);
    assert!(diff.reason.contains("count"), "got: {}", diff.reason);
}

#[test]
fn detects_kind_change() {
    let diff = divergence(LuaVersion::Lua54, "local x = 1", "x = 1").expect("should diverge");
    assert!(diff.reason.contains("kind"));
}

#[test]
fn identical_type_aliases_are_equivalent() {
    let alias = "type Pair = { first: number, second: string }";
    assert_eq!(divergence(LuaVersion::Luau, alias, alias), None);
}

#[test]
fn string_singleton_quote_style_is_equivalent() {
    // Singletons compare by decoded content, so a quote swap still matches.
    let diff = divergence(LuaVersion::Luau, "type State = \"on\"", "type State = 'on'");
    assert_eq!(diff, None);
}

#[test]
fn redundant_type_parens_are_equivalent() {
    let diff = divergence(LuaVersion::Luau, "local x: (number) = 1", "local x: number = 1");
    assert_eq!(diff, None);
}

#[test]
fn leading_union_pipe_is_equivalent() {
    // A leading `|` is trivia the formatter may add or remove.
    let diff = divergence(
        LuaVersion::Luau,
        "type Value = | number | string",
        "type Value = number | string",
    );
    assert_eq!(diff, None);
}

#[test]
fn detects_differing_annotation_type() {
    let diff = divergence(LuaVersion::Luau, "local x: number = 1", "local x: string = 1")
        .expect("differing types should diverge");
    assert!(diff.path.contains("type"), "got path: {}", diff.path);
}

#[test]
fn detects_dropped_annotation() {
    // A dropped annotation is data loss, never a tolerated rewrite.
    let diff = divergence(LuaVersion::Luau, "local x: number = 1", "local x = 1")
        .expect("dropped annotation should diverge");
    assert!(diff.reason.contains("presence differs"), "got: {}", diff.reason);
}

#[test]
fn detects_a_changed_interpolated_string() {
    for (left, right) in
        [("local x = `{a}`", "local x = `{b}`"), ("local x = `one`", "local x = `two`")]
    {
        assert!(
            divergence(LuaVersion::Luau, left, right).is_some(),
            "{left:?} and {right:?} are different programs"
        );
    }
}

#[test]
fn detects_a_changed_compound_operator() {
    assert!(
        divergence(LuaVersion::Luau, "x += 1", "x -= 1").is_some(),
        "`+=` and `-=` are different programs"
    );
}

#[test]
fn detects_a_dropped_type_export() {
    assert!(
        divergence(LuaVersion::Luau, "export type A = number", "type A = number").is_some(),
        "dropping `export` changes the module's public types"
    );
}

#[test]
fn detects_a_renamed_vararg() {
    assert!(
        divergence(
            LuaVersion::Lua55,
            "function f(...a) return a end",
            "function f(...b) return a end"
        )
        .is_some(),
        "renaming a named vararg changes what the body reads"
    );
}

#[test]
fn detects_a_changed_global_star_attribute() {
    assert!(
        divergence(LuaVersion::Lua55, "global <const> *", "global *").is_some(),
        "`global <const> *` and `global *` declare different access"
    );
}

#[test]
fn detects_dropped_value_truncating_parens() {
    assert!(
        divergence(LuaVersion::Lua54, "return (f())", "return f()").is_some(),
        "`(f())` truncates to one value, so these parens are not redundant"
    );
}
