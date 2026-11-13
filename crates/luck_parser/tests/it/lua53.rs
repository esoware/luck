use luck_ast::Expression;
use luck_parser::ParseResult;
use luck_token::{BinOp, LuaVersion, UnOp};

use crate::common::{assert_no_errors, local_value, run_operands, run_ops};

fn parse_lua53(source: &str) -> ParseResult {
    luck_parser::parse(source, LuaVersion::Lua53)
}

#[test]
fn bitwise_and_or_precedence() {
    // `&` binds tighter than `|`, so `a & b | c` is `(a & b) | c`.
    let result = parse_lua53("local x = a & b | c");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::BitOr]);
    assert_eq!(run_ops(run_operands(expr)[0]), [BinOp::BitAnd]);
}

#[test]
fn unary_bitwise_not() {
    let result = parse_lua53("local x = ~y");
    assert_no_errors(&result);
    let expr = local_value(&result);
    if let Expression::UnaryOp(unary) = expr {
        assert!(matches!(unary.op, UnOp::BitNot));
    } else {
        panic!("expected UnaryOp, got {expr:?}");
    }
}

#[test]
fn floor_division() {
    let result = parse_lua53("local x = a // b");
    assert_no_errors(&result);
    assert_eq!(run_ops(local_value(&result)), [BinOp::FloorDiv]);
}

#[test]
fn left_shift() {
    let result = parse_lua53("local x = a << 3");
    assert_no_errors(&result);
    assert_eq!(run_ops(local_value(&result)), [BinOp::Shl]);
}

#[test]
fn shift_and_bitwise_precedence() {
    // `>>` (level 7) binds tighter than `&` (level 6), so `a >> b & c`
    // parses as `(a >> b) & c`.
    let result = parse_lua53("local x = a >> b & c");
    assert_no_errors(&result);
    let expr = local_value(&result);
    assert_eq!(run_ops(expr), [BinOp::BitAnd]);
    assert_eq!(run_ops(run_operands(expr)[0]), [BinOp::Shr]);
}

#[test]
fn bitwise_xor() {
    let result = parse_lua53("local x = a ~ b");
    assert_no_errors(&result);
    assert_eq!(run_ops(local_value(&result)), [BinOp::BitXor]);
}

#[test]
fn goto_and_labels_parse_in_lua53() {
    let result = parse_lua53("goto done ::done::");
    assert_no_errors(&result);
    assert_eq!(result.block.stmts.len(), 2);
}
