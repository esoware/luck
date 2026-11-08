use luck_ast::{Expression, Statement};
use luck_parser::ParseResult;
use luck_token::LuaVersion;

pub(crate) fn assert_no_errors(result: &ParseResult) {
    assert!(result.errors.is_empty(), "parse errors: {:?}", result.errors);
}

pub(crate) fn parse_luau(source: &str) -> ParseResult {
    luck_parser::parse(source, LuaVersion::Luau)
}

/// The last initializer of a `local ... = <expr>` first statement.
pub(crate) fn local_value(result: &ParseResult) -> &Expression {
    let Some(Statement::LocalAssignment(local)) = result.block.stmts.first() else {
        panic!("expected LocalAssignment, got {:?}", result.block.stmts);
    };
    let exprs = local.exprs.as_ref().expect("assignment has values");
    exprs.last().expect("expression list has last element")
}
