use core::fmt::Write as _;
use core::iter;

use luck_ast::{Expression, Statement, Suffix, SuffixedExpression, Var};
use luck_parser::ParseResult;
use luck_token::{BinOp, LuaVersion};

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

/// A binary run's operators, in source order.
pub(crate) fn run_ops(expr: &Expression) -> Vec<BinOp> {
    let Expression::BinaryOp(run) = expr else {
        panic!("expected a binary run, got {expr:?}");
    };
    run.links.iter().map(|link| link.op).collect()
}

/// A binary run's operands, in source order.
pub(crate) fn run_operands(expr: &Expression) -> Vec<&Expression> {
    let Expression::BinaryOp(run) = expr else {
        panic!("expected a binary run, got {expr:?}");
    };
    iter::once(&run.first).chain(run.links.iter().map(|link| &link.operand)).collect()
}

pub(crate) fn chain(expr: &Expression) -> &SuffixedExpression {
    let Expression::Suffixed(chain) = expr else {
        panic!("expected a suffixed expression, got {expr:?}");
    };
    chain
}

pub(crate) fn target_chain(var: &Var) -> &SuffixedExpression {
    let Var::Suffixed(chain) = var else {
        panic!("expected a suffixed target, got {var:?}");
    };
    chain
}

/// The prefix's name, or `()` for parentheses, then each suffix: `.name`,
/// `[]`, `()`, `:name()`, or `<<>>`.
pub(crate) fn chain_shape(chain: &SuffixedExpression) -> String {
    let mut shape = if let Expression::Name(name) = &chain.prefix {
        name.identifier().unwrap_or_default().to_owned()
    } else if let Expression::Parenthesized(_) = &chain.prefix {
        "()".to_owned()
    } else {
        format!("{:?}", chain.prefix)
    };
    for suffix in &chain.suffixes {
        let _unused = match suffix {
            Suffix::Field(name) => write!(shape, ".{}", name.identifier().unwrap_or_default()),
            Suffix::Index { .. } => write!(shape, "[]"),
            Suffix::Call(call) => match &call.method {
                Some(method) => write!(shape, ":{}()", method.identifier().unwrap_or_default()),
                None => write!(shape, "()"),
            },
            Suffix::TypeInstantiation { .. } => write!(shape, "<<>>"),
        };
    }
    shape
}
