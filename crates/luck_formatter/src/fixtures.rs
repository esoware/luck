use luck_allocator::Allocator;
use luck_ast::Statement;
use luck_token::LuaVersion;

use crate::FormatOptions;
use crate::comments::Comments;
use crate::ir::Formatter;

/// What `write` writes, printed with no comments.
pub(crate) fn render<'ast>(
    allocator: &'ast Allocator,
    options: FormatOptions,
    write: impl FnOnce(&mut Formatter<'ast>),
) -> String {
    let mut f = Formatter::new(allocator, LuaVersion::Luau, options, Comments::none());
    write(&mut f);
    String::from_utf8(f.print(b"")).unwrap()
}

/// The Luau expression `source`, printed alone.
pub(crate) fn render_expr(source: &str, options: FormatOptions) -> String {
    let allocator = Allocator::new();
    let text = allocator.alloc_str(&format!("return {source}\n"));
    let parsed = luck_parser::parse(&allocator, text.as_bytes(), LuaVersion::Luau);
    assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
    let [Statement::Return(return_stmt)] = &*parsed.chunk.block.stmts else {
        panic!("expected a return statement");
    };
    let [expr] = &*return_stmt.exprs else {
        panic!("expected one returned expression");
    };
    let mut f = Formatter::new(&allocator, LuaVersion::Luau, options, Comments::none())
        .with_groupings(&parsed.groupings);
    f.write_expr(expr);
    String::from_utf8(f.print(b"")).unwrap()
}
