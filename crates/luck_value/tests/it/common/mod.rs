use luck_allocator::Allocator;
use luck_ast::{Expression, Name, Statement};
use luck_token::LuaVersion;
use luck_value::ValueContext;

/// A dialect in which the names `locals` lists are locals, and every other
/// a global.
pub(crate) struct Locals {
    pub version: LuaVersion,
    pub locals: &'static [&'static str],
}

impl ValueContext for Locals {
    fn version(&self) -> LuaVersion {
        self.version
    }

    fn is_global_reference(&self, name: &Name<'_>) -> bool {
        !self.locals.contains(&name.text.as_str())
    }
}

/// What `answer` says of the one expression `return {source}` returns, in
/// `version`.
pub(crate) fn of_expr<T>(
    source: &str,
    version: LuaVersion,
    answer: impl FnOnce(&Expression<'_>) -> T,
) -> T {
    let allocator = Allocator::new();
    let text = format!("return {source}");
    let parsed = luck_parser::parse(&allocator, text.as_bytes(), version);
    assert!(parsed.errors.is_empty(), "{source} must parse: {:?}", parsed.errors);
    let Some(Statement::Return(stmt)) = parsed.chunk.block.stmts.first() else {
        panic!("the chunk is one return");
    };
    let [expr] = &*stmt.exprs else { panic!("{source} must be one expression") };
    answer(expr)
}
