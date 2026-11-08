//! Helpers the unit tests share.

use luck_ast::{Expression, LastStatement};
use luck_token::LuaVersion;

use crate::FormatOptions;
use crate::comments::Comments;
use crate::ir::{Format, Formatter};

/// Lay out `node` on its own, with no comments.
pub(crate) fn render(node: &dyn Format, options: FormatOptions) -> String {
    let mut formatter = Formatter::new(options, Comments::none(), None);
    node.fmt(&mut formatter);
    formatter.print()
}

/// The expression `source` parses to, read as Luau.
pub(crate) fn parse_expr(source: &str) -> Expression {
    let parsed = luck_parser::parse(format!("return {source}\n"), LuaVersion::Luau);
    assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
    let last_stmt = parsed.block.last_stmt.expect("wrapped return statement");
    let LastStatement::Return(return_stmt) = *last_stmt else {
        panic!("expected a return statement");
    };
    return_stmt.exprs.items.into_iter().next().expect("one returned expression")
}
