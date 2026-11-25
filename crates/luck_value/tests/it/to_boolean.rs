use luck_token::LuaVersion;
use luck_value::ToBoolean as _;

use crate::common::of_expr;

fn assert_truthiness(version: LuaVersion, cases: &[(&str, Option<bool>)]) {
    for &(source, expected) in cases {
        of_expr(source, version, |expr| {
            assert_eq!(expr.to_boolean(&version), expected, "{source} on {version:?}");
        });
    }
}

#[test]
fn only_nil_and_false_are_falsy() {
    assert_truthiness(
        LuaVersion::Lua54,
        &[
            ("nil", Some(false)),
            ("false", Some(false)),
            ("true", Some(true)),
            ("0", Some(true)),
            ("''", Some(true)),
            ("{}", Some(true)),
            ("function() end", Some(true)),
            ("(nil)", Some(false)),
        ],
    );
    assert_truthiness(LuaVersion::Luau, &[("0i", Some(true)), ("`text`", Some(true))]);
}

#[test]
fn a_value_reached_through_an_effect_keeps_its_truthiness() {
    assert_truthiness(
        LuaVersion::Lua54,
        &[("{f()}", Some(true)), ("x or true", Some(true)), ("x and false", Some(false))],
    );
}

#[test]
fn an_operator_on_values_of_known_types_yields_a_value_of_its_type() {
    assert_truthiness(
        LuaVersion::Lua54,
        &[
            ("1 + 2", Some(true)),
            ("0.3 ^ 3", Some(true)),
            ("'a' .. 1", Some(true)),
            ("#'abc'", Some(true)),
            ("not x", None),
            ("not not 1", Some(true)),
            ("1 < 2", Some(true)),
            ("'a' < 'b'", None),
        ],
    );
}

#[test]
fn a_metamethod_can_return_anything() {
    assert_truthiness(
        LuaVersion::Lua54,
        &[
            ("x + 1", None),
            ("x .. 'a'", None),
            ("#t", None),
            ("x == y", None),
            ("x", None),
            ("...", None),
            ("f()", None),
        ],
    );
    // Luau: an interpolated string with an expression calls the `format`
    // method strings find through their metatable.
    assert_truthiness(LuaVersion::Luau, &[("`a{x}`", None)]);
}

#[test]
fn an_if_expression_has_the_truthiness_its_reachable_branches_share() {
    assert_truthiness(
        LuaVersion::Luau,
        &[
            ("if x then 1 else 'a'", Some(true)),
            ("if x then nil else false", Some(false)),
            ("if x then 1 else nil", None),
            ("if true then 1 else nil", Some(true)),
            ("if false then 1 elseif x then nil else false", Some(false)),
        ],
    );
}
