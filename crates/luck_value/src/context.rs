use luck_ast::Name;
use luck_token::LuaVersion;

/// What the analysis has to know of where an expression is: the dialect,
/// whose arithmetic evaluation reproduces, and which names are globals.
pub trait ValueContext {
    fn version(&self) -> LuaVersion;

    /// Whether `name` reads a global. A global read indexes `_ENV` or the
    /// function's environment, which can run `__index`, and a read of a local
    /// or an upvalue runs nothing.
    fn is_global_reference(&self, name: &Name<'_>) -> bool;
}

/// A dialect knows no bindings, so it takes every name for a global.
impl ValueContext for LuaVersion {
    fn version(&self) -> LuaVersion {
        *self
    }

    fn is_global_reference(&self, _name: &Name<'_>) -> bool {
        true
    }
}
