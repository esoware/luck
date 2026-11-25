use luck_ast::Name;

/// What the analysis has to know of where an expression is: which names are
/// globals.
pub trait ValueContext {
    /// Whether `name` reads a global. A global read indexes the function's
    /// environment, which can run `__index`, and a read of a local or an
    /// upvalue runs nothing.
    fn is_global_reference(&self, name: &Name<'_>) -> bool;
}

/// A context that knows no bindings, so it takes every name for a global.
#[derive(Debug, Clone, Copy)]
pub struct NoBindings;

impl ValueContext for NoBindings {
    fn is_global_reference(&self, _name: &Name<'_>) -> bool {
        true
    }
}
