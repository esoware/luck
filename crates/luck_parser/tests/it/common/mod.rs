use core::fmt::Write as _;
use core::iter;

use luck_allocator::Allocator;
use luck_ast::{Expression, Statement, Suffix, SuffixedExpression, Var};
use luck_parser::ParseResult;
use luck_token::{BinOp, LuaVersion};

pub(crate) fn assert_no_errors(result: &ParseResult<'_>) {
    assert!(result.errors.is_empty(), "parse errors: {:?}", result.errors);
}

pub(crate) fn parse_luau<'ast>(allocator: &'ast Allocator, source: &'ast str) -> ParseResult<'ast> {
    luck_parser::parse(allocator, source, LuaVersion::Luau)
}

/// The last initializer of a `local ... = <expr>` first statement.
pub(crate) fn local_value<'ast>(result: &'ast ParseResult<'ast>) -> &'ast Expression<'ast> {
    let Some(Statement::LocalAssignment(local)) = result.chunk.block.stmts.first() else {
        panic!("expected LocalAssignment, got {:?}", result.chunk.block.stmts);
    };
    local.exprs.last().expect("assignment has values")
}

/// A binary run's operators, in source order.
pub(crate) fn run_ops(expr: &Expression<'_>) -> Vec<BinOp> {
    let Expression::BinaryOp(run) = expr else {
        panic!("expected a binary run, got {expr:?}");
    };
    run.links.iter().map(|link| link.op).collect()
}

/// A binary run's operands, in source order.
pub(crate) fn run_operands<'tree, 'ast>(
    expr: &'tree Expression<'ast>,
) -> Vec<&'tree Expression<'ast>> {
    let Expression::BinaryOp(run) = expr else {
        panic!("expected a binary run, got {expr:?}");
    };
    iter::once(&run.first).chain(run.links.iter().map(|link| &link.operand)).collect()
}

pub(crate) fn chain<'tree, 'ast>(expr: &'tree Expression<'ast>) -> &'tree SuffixedExpression<'ast> {
    let Expression::Suffixed(chain) = expr else {
        panic!("expected a suffixed expression, got {expr:?}");
    };
    chain
}

pub(crate) fn target_chain<'tree, 'ast>(var: &'tree Var<'ast>) -> &'tree SuffixedExpression<'ast> {
    let Var::Suffixed(chain) = var else {
        panic!("expected a suffixed target, got {var:?}");
    };
    chain
}

/// The prefix's name, or `()` for parentheses, then each suffix: `.name`,
/// `[]`, `()`, `:name()`, or `<<>>`.
pub(crate) fn chain_shape(chain: &SuffixedExpression<'_>) -> String {
    let mut shape = if let Expression::Name(name) = &chain.prefix {
        name.text.to_string()
    } else if let Expression::Parenthesized(_) = &chain.prefix {
        "()".to_owned()
    } else {
        format!("{:?}", chain.prefix)
    };
    for suffix in &chain.suffixes {
        let _unused = match suffix {
            Suffix::Field(name) => write!(shape, ".{}", name.text),
            Suffix::Index { .. } => write!(shape, "[]"),
            Suffix::Call(_) => write!(shape, "()"),
            Suffix::MethodCall(call) => write!(shape, ":{}()", call.name.text),
            Suffix::TypeInstantiation { .. } => write!(shape, "<<>>"),
        };
    }
    shape
}
