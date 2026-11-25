use luck_token::{BinOp, LuaNumber, LuaVersion};
use luck_value::{ConstantEvaluation as _, ConstantValue};

use crate::common::{Locals, of_expr};

const INT: fn(i64) -> Option<ConstantValue<'static>> =
    |integer| Some(ConstantValue::Number(LuaNumber::Int(integer)));
const FLOAT: fn(f64) -> Option<ConstantValue<'static>> =
    |float| Some(ConstantValue::Number(LuaNumber::Float(float)));
const BOOLEAN: fn(bool) -> Option<ConstantValue<'static>> =
    |boolean| Some(ConstantValue::Boolean(boolean));

fn assert_values(version: LuaVersion, cases: &[(&str, Option<ConstantValue<'static>>)]) {
    for &(source, expected) in cases {
        of_expr(source, version, |expr| {
            assert_eq!(expr.evaluate_value(&version), expected, "{source} on {version:?}");
        });
    }
}

fn is_nan(source: &str, version: LuaVersion) -> bool {
    of_expr(source, version, |expr| {
        matches!(
            expr.evaluate_value(&version),
            Some(ConstantValue::Number(LuaNumber::Float(float))) if float.is_nan()
        )
    })
}

#[test]
fn literals_are_their_values() {
    assert_values(
        LuaVersion::Lua54,
        &[
            ("nil", Some(ConstantValue::Nil)),
            ("true", BOOLEAN(true)),
            ("0x10", INT(16)),
            ("1.5", FLOAT(1.5)),
            ("'\\65\\66'", Some(ConstantValue::String(b"AB"))),
            ("(((1)))", INT(1)),
        ],
    );
    assert_values(
        LuaVersion::Luau,
        &[
            ("1i", Some(ConstantValue::Integer(1))),
            ("`text`", Some(ConstantValue::String(b"text"))),
            ("(2 :: number)", FLOAT(2.0)),
        ],
    );
}

#[test]
fn integers_wrap_and_floor_from_lua53() {
    assert_values(
        LuaVersion::Lua54,
        &[
            ("7 // 2", INT(3)),
            ("-7 // 2", INT(-4)),
            ("7 % -3", INT(-2)),
            ("0x7fffffffffffffff + 1", INT(i64::MIN)),
            ("0x8000000000000000 // -1", INT(i64::MIN)),
            ("0x8000000000000000 % -1", INT(0)),
            ("- 0x8000000000000000", INT(i64::MIN)),
            ("7 / 2", FLOAT(3.5)),
            ("#'abc'", INT(3)),
            ("1 // 0", None),
            ("1 % 0", None),
        ],
    );
    assert_values(LuaVersion::Lua51, &[("7 / 2", FLOAT(3.5)), ("#'abc'", FLOAT(3.0))]);
}

#[test]
fn float_modulo_follows_each_version() {
    assert_values(LuaVersion::Lua54, &[("1e16 % 3", FLOAT(1.0)), ("-6.0 % 3.0", FLOAT(-0.0))]);
    assert_values(LuaVersion::Lua51, &[("1e16 % 3", FLOAT(0.0)), ("-6.0 % 3.0", FLOAT(0.0))]);
}

#[test]
fn a_float_divided_by_zero_is_infinite_or_nan() {
    assert_values(
        LuaVersion::Lua54,
        &[("1 / 0", FLOAT(f64::INFINITY)), ("1.0 // 0", FLOAT(f64::INFINITY))],
    );
    for source in ["0 / 0", "1.0 % 0", "1e999 - 1e999"] {
        assert!(is_nan(source, LuaVersion::Lua54), "{source}");
    }
}

#[test]
fn results_the_target_computes_its_own_way_have_no_value() {
    assert_values(
        LuaVersion::Lua54,
        &[
            ("2 ^ 10", FLOAT(1024.0)),
            ("0.3 ^ 3", None),
            ("2 ^ 0.5", None),
            ("'a' < 'b'", None),
            ("'a' .. 'b'", None),
            ("1 .. ''", None),
            ("-0.0 - 0", None),
        ],
    );
}

#[test]
fn bitwise_operators_take_integers_and_floats_with_their_value() {
    assert_values(
        LuaVersion::Lua54,
        &[
            ("6 & 3", INT(2)),
            ("6 | 1.0", INT(7)),
            ("6 ~ 3", INT(5)),
            ("~5", INT(-6)),
            ("1 << 63", INT(i64::MIN)),
            ("1 << 64", INT(0)),
            ("-1 >> 1", INT(i64::MAX)),
            ("1 >> -2", INT(4)),
            ("1 << 0x8000000000000000", INT(0)),
            ("1 | 1.5", None),
            ("1 | '1'", None),
        ],
    );
}

#[test]
fn comparisons_are_exact_and_false_with_nan() {
    assert_values(
        LuaVersion::Lua54,
        &[
            ("1 < 2", BOOLEAN(true)),
            ("2 <= 2", BOOLEAN(true)),
            ("2 > 1.5", BOOLEAN(true)),
            ("1 == 1.0", BOOLEAN(true)),
            ("9007199254740993 == 9007199254740992.0", BOOLEAN(false)),
            ("0.0 == -0.0", BOOLEAN(true)),
            ("0/0 == 0/0", BOOLEAN(false)),
            ("0/0 ~= 0/0", BOOLEAN(true)),
            ("0/0 < 1", BOOLEAN(false)),
            ("0/0 >= 1", BOOLEAN(false)),
            ("nil == false", BOOLEAN(false)),
            ("1 == '1'", BOOLEAN(false)),
            ("'a' == '\\97'", BOOLEAN(true)),
            ("1 < '2'", None),
        ],
    );
    assert_values(LuaVersion::Luau, &[("1i == 1i", BOOLEAN(true)), ("1i == 1", BOOLEAN(false))]);
}

#[test]
fn logical_operators_yield_the_operand_they_pick() {
    assert_values(
        LuaVersion::Lua54,
        &[
            ("nil or 1", INT(1)),
            ("false and x", BOOLEAN(false)),
            ("1 or x", INT(1)),
            ("not nil", BOOLEAN(true)),
            ("not 0", BOOLEAN(false)),
            ("not {}", BOOLEAN(false)),
            ("x and 1", None),
            ("true and x", None),
        ],
    );
    assert_values(
        LuaVersion::Luau,
        &[("if true then 1 else x", FLOAT(1.0)), ("if nil then x else 2", FLOAT(2.0))],
    );
}

#[test]
fn a_value_reached_through_an_effect_has_none() {
    assert_values(
        LuaVersion::Lua54,
        &[("{f()} and 1", None), ("x == nil", None), ("#t", None), ("-x", None)],
    );
    let context = Locals { version: LuaVersion::Lua54, locals: &["x"] };
    of_expr("x and nil or 1", LuaVersion::Lua54, |expr| {
        assert_eq!(expr.evaluate_value(&context), INT(1), "a local's read has no effect");
    });
}

#[test]
fn the_side_free_truthiness_of_a_pure_expression_is_known_without_its_value() {
    let context = Locals { version: LuaVersion::Lua54, locals: &["x"] };
    for (source, expected) in [
        ("{}", Some(true)),
        ("function() end", Some(true)),
        ("x or 'y'", Some(true)),
        ("x and nil", Some(false)),
        ("{f()}", None),
        ("g or true", None),
        ("x", None),
    ] {
        of_expr(source, LuaVersion::Lua54, |expr| {
            assert_eq!(expr.get_side_free_boolean_value(&context), expected, "{source}");
        });
    }
}

#[test]
fn a_binary_operator_applies_to_two_values() {
    let one = ConstantValue::Number(LuaNumber::Int(1));
    let half = ConstantValue::Number(LuaNumber::Float(0.5));
    let version = LuaVersion::Lua54;
    assert_eq!(one.binary(BinOp::Add, half, version), FLOAT(1.5));
    assert_eq!(one.binary(BinOp::Shl, one, version), INT(2));
    assert_eq!(one.binary(BinOp::Shl, one, LuaVersion::Lua51), None);
    assert_eq!(ConstantValue::Nil.binary(BinOp::Or, one, version), Some(one));
    assert_eq!(one.binary(BinOp::Concat, one, version), None);
}
