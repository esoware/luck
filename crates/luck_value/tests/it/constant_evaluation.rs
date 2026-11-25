use luck_token::BinOp;
use luck_value::{ConstantEvaluation as _, ConstantValue, NoBindings};

use crate::common::{Locals, of_expr};

const NUMBER: fn(f64) -> Option<ConstantValue<'static>> =
    |number| Some(ConstantValue::Number(number));
const BOOLEAN: fn(bool) -> Option<ConstantValue<'static>> =
    |boolean| Some(ConstantValue::Boolean(boolean));

fn assert_values(cases: &[(&str, Option<ConstantValue<'static>>)]) {
    for &(source, expected) in cases {
        of_expr(source, |expr| {
            assert_eq!(expr.evaluate_value(&NoBindings), expected, "{source}");
        });
    }
}

fn is_nan(source: &str) -> bool {
    of_expr(source, |expr| {
        matches!(
            expr.evaluate_value(&NoBindings),
            Some(ConstantValue::Number(number)) if number.is_nan()
        )
    })
}

#[test]
fn literals_are_their_values() {
    assert_values(&[
        ("nil", Some(ConstantValue::Nil)),
        ("true", BOOLEAN(true)),
        ("0x10", NUMBER(16.0)),
        ("1.5", NUMBER(1.5)),
        ("'\\65\\66'", Some(ConstantValue::String(b"AB"))),
        ("(((1)))", NUMBER(1.0)),
        ("`text`", Some(ConstantValue::String(b"text"))),
        ("(2 :: number)", NUMBER(2.0)),
    ]);
}

#[test]
fn every_number_is_a_float() {
    assert_values(&[
        ("7 // 2", NUMBER(3.0)),
        ("-7 // 2", NUMBER(-4.0)),
        ("7 % -3", NUMBER(-2.0)),
        ("7 / 2", NUMBER(3.5)),
        ("#'abc'", NUMBER(3.0)),
        ("-0.0 - 0", NUMBER(-0.0)),
    ]);
}

#[test]
fn float_modulo_subtracts_the_floored_quotient() {
    assert_values(&[("1e16 % 3", NUMBER(0.0)), ("-6.0 % 3.0", NUMBER(0.0))]);
}

#[test]
fn a_division_by_zero_is_infinite_or_nan() {
    assert_values(&[
        ("1 / 0", NUMBER(f64::INFINITY)),
        ("1 // 0", NUMBER(f64::INFINITY)),
        ("-1 // 0", NUMBER(f64::NEG_INFINITY)),
    ]);
    for source in ["0 / 0", "1 % 0", "1e999 - 1e999"] {
        assert!(is_nan(source), "{source}");
    }
}

#[test]
fn results_the_runtime_computes_its_own_way_have_no_value() {
    assert_values(&[
        ("2 ^ 10", NUMBER(1024.0)),
        ("0.3 ^ 3", None),
        ("2 ^ 0.5", None),
        ("'a' < 'b'", None),
        ("'a' .. 'b'", None),
        ("1 .. ''", None),
    ]);
}

#[test]
fn comparisons_are_exact_and_false_with_nan() {
    assert_values(&[
        ("1 < 2", BOOLEAN(true)),
        ("2 <= 2", BOOLEAN(true)),
        ("2 > 1.5", BOOLEAN(true)),
        ("1 == 1.0", BOOLEAN(true)),
        ("0.0 == -0.0", BOOLEAN(true)),
        ("0/0 == 0/0", BOOLEAN(false)),
        ("0/0 ~= 0/0", BOOLEAN(true)),
        ("0/0 < 1", BOOLEAN(false)),
        ("0/0 >= 1", BOOLEAN(false)),
        ("nil == false", BOOLEAN(false)),
        ("1 == '1'", BOOLEAN(false)),
        ("'a' == '\\97'", BOOLEAN(true)),
        ("1 < '2'", None),
    ]);
}

#[test]
fn logical_operators_yield_the_operand_they_pick() {
    assert_values(&[
        ("nil or 1", NUMBER(1.0)),
        ("false and x", BOOLEAN(false)),
        ("1 or x", NUMBER(1.0)),
        ("not nil", BOOLEAN(true)),
        ("not 0", BOOLEAN(false)),
        ("not {}", BOOLEAN(false)),
        ("x and 1", None),
        ("true and x", None),
        ("if true then 1 else x", NUMBER(1.0)),
        ("if nil then x else 2", NUMBER(2.0)),
    ]);
}

#[test]
fn a_value_reached_through_an_effect_has_none() {
    assert_values(&[("{f()} and 1", None), ("x == nil", None), ("#t", None), ("-x", None)]);
    let context = Locals { locals: &["x"] };
    of_expr("x and nil or 1", |expr| {
        assert_eq!(expr.evaluate_value(&context), NUMBER(1.0), "a local's read has no effect");
    });
}

#[test]
fn the_side_free_truthiness_of_a_pure_expression_is_known_without_its_value() {
    let context = Locals { locals: &["x"] };
    for (source, expected) in [
        ("{}", Some(true)),
        ("function() end", Some(true)),
        ("x or 'y'", Some(true)),
        ("x and nil", Some(false)),
        ("{f()}", None),
        ("g or true", None),
        ("x", None),
    ] {
        of_expr(source, |expr| {
            assert_eq!(expr.get_side_free_boolean_value(&context), expected, "{source}");
        });
    }
}

#[test]
fn a_binary_operator_applies_to_two_values() {
    let one = ConstantValue::Number(1.0);
    let half = ConstantValue::Number(0.5);
    assert_eq!(one.binary(BinOp::Add, half), NUMBER(1.5));
    assert_eq!(one.binary(BinOp::Lt, half), BOOLEAN(false));
    assert_eq!(ConstantValue::Nil.binary(BinOp::Or, one), Some(one));
    assert_eq!(one.binary(BinOp::Concat, one), None);
}
