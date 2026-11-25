use luck_allocator::Allocator;
use luck_ast::Statement;
use luck_parser::ParseResult;

use crate::FormatOptions;
use crate::comments::Comments;
use crate::ir::Formatter;

/// `source` parsed, its tokens kept, whether it parses or not.
pub(crate) fn parse<'ast>(allocator: &'ast Allocator, source: &'ast str) -> ParseResult<'ast> {
    luck_parser::parse_with_tokens(allocator, source.as_bytes())
}

/// What `write` writes, printed with no comments.
pub(crate) fn render<'ast>(
    allocator: &'ast Allocator,
    options: FormatOptions,
    write: impl FnOnce(&mut Formatter<'ast>),
) -> String {
    let mut f = Formatter::new(allocator, options, Comments::none(), None);
    write(&mut f);
    String::from_utf8(f.print(b"")).unwrap()
}

/// The expression `source`, printed alone.
pub(crate) fn render_expr(source: &str, options: FormatOptions) -> String {
    let allocator = Allocator::new();
    let text = allocator.alloc_str(&format!("return {source}\n"));
    let parsed = luck_parser::parse(&allocator, text.as_bytes());
    assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
    let [Statement::Return(return_stmt)] = &*parsed.chunk.stmts else {
        panic!("expected a return statement");
    };
    let [expr] = &*return_stmt.exprs else {
        panic!("expected one returned expression");
    };
    let mut f = Formatter::new(&allocator, options, Comments::none(), Some(&parsed.groupings));
    f.write_expr(expr);
    String::from_utf8(f.print(b"")).unwrap()
}
