use luck_token::*;

use crate::common::first_kind;

#[test]
fn number_integer() {
    assert_eq!(first_kind("42"), TokenKind::Number("42".into()));
}

#[test]
fn number_float() {
    assert_eq!(first_kind("3.14"), TokenKind::Number("3.14".into()));
}

#[test]
fn number_leading_dot() {
    assert_eq!(first_kind(".5"), TokenKind::Number(".5".into()));
}

#[test]
fn number_exponent() {
    assert_eq!(first_kind("1e10"), TokenKind::Number("1e10".into()));
}

#[test]
fn number_exponent_negative() {
    assert_eq!(first_kind("3.14e-2"), TokenKind::Number("3.14e-2".into()));
}

#[test]
fn number_exponent_positive() {
    assert_eq!(first_kind("1E+5"), TokenKind::Number("1E+5".into()));
}

#[test]
fn number_hex_lower() {
    assert_eq!(first_kind("0xFF"), TokenKind::Number("0xFF".into()));
}

#[test]
fn number_hex_upper() {
    assert_eq!(first_kind("0XAB"), TokenKind::Number("0XAB".into()));
}
