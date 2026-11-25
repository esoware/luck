use luck_value::{NoBindings, ToBoolean as _};

use crate::common::of_expr;

fn assert_truthiness(cases: &[(&str, Option<bool>)]) {
    for &(source, expected) in cases {
        of_expr(source, |expr| {
            assert_eq!(expr.to_boolean(&NoBindings), expected, "{source}");
        });
    }
}

#[test]
fn only_nil_and_false_are_falsy() {
    assert_truthiness(&[
        ("nil", Some(false)),
        ("false", Some(false)),
        ("true", Some(true)),
        ("0", Some(true)),
        ("0i", Some(true)),
        ("''", Some(true)),
        ("`text`", Some(true)),
        ("{}", Some(true)),
        ("function() end", Some(true)),
        ("(nil)", Some(false)),
    ]);
}

#[test]
fn a_value_reached_through_an_effect_keeps_its_truthiness() {
    assert_truthiness(&[
        ("{f()}", Some(true)),
        ("x or true", Some(true)),
        ("x and false", Some(false)),
    ]);
}

#[test]
fn an_operator_on_values_of_known_types_yields_a_value_of_its_type() {
    assert_truthiness(&[
        ("1 + 2", Some(true)),
        ("0.3 ^ 3", Some(true)),
        ("'a' .. 1", Some(true)),
        ("#'abc'", Some(true)),
        ("not x", None),
        ("not not 1", Some(true)),
        ("1 < 2", Some(true)),
        ("'a' < 'b'", None),
    ]);
}

#[test]
fn a_metamethod_can_return_anything() {
    assert_truthiness(&[
        ("x + 1", None),
        ("x .. 'a'", None),
        ("#t", None),
        ("x == y", None),
        ("x", None),
        ("...", None),
        ("f()", None),
        // An expression in an interpolated string calls the `format` method
        // strings find through their metatable.
        ("`a{x}`", None),
    ]);
}

#[test]
fn an_if_expression_has_the_truthiness_its_reachable_branches_share() {
    assert_truthiness(&[
        ("if x then 1 else 'a'", Some(true)),
        ("if x then nil else false", Some(false)),
        ("if x then 1 else nil", None),
        ("if true then 1 else nil", Some(true)),
        ("if false then 1 elseif x then nil else false", Some(false)),
    ]);
}
