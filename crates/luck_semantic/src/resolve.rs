//! Conservative stdlib resolution: maps call sites and value reads to
//! `StdlibEntry`s, including colon calls on shaped values such as
//! `game:GetService(...)` and `f:read()` after `local f = io.open(...)`.
//!
//! There's no type inference. A value has a known shape only when it's a
//! stdlib value that declares one, a string literal, a call whose entry
//! declares a return shape, or a never-reassigned local initialized from
//! one of those.

use luck_ast::{
    Block, DottedPath, Expression, FunctionCall, Statement, Var, Visitor, dotted_path,
    ensure_sufficient_stack,
};
use luck_token::{CompactString, Span};
use rustc_hash::FxHashMap;

use crate::scope::{ScopeTree, SymbolId};
use crate::stdlib_model::{StdlibEntry, StdlibLibrary};

/// The shapes of never-reassigned locals, by symbol.
pub(crate) type SymbolShapes = FxHashMap<SymbolId, CompactString>;

/// What shape resolution reads: the names in scope, the library, and the
/// local shapes known so far.
#[derive(Clone, Copy)]
pub(crate) struct ShapeResolver<'input> {
    tree: &'input ScopeTree,
    library: &'input StdlibLibrary,
    symbol_shapes: &'input SymbolShapes,
}

impl<'input> ShapeResolver<'input> {
    pub(crate) const fn new(
        tree: &'input ScopeTree,
        library: &'input StdlibLibrary,
        symbol_shapes: &'input SymbolShapes,
    ) -> Self {
        Self { tree, library, symbol_shapes }
    }

    /// The shape of `expr`, when statically known. See the module docs for
    /// what "known" means.
    pub(crate) fn expr_shape(self, expr: &Expression) -> Option<&'input CompactString> {
        match expr {
            Expression::Var(Var::Name(token)) => {
                let name = token.identifier()?;
                match self.tree.resolved_symbol(name, token.span) {
                    Some(symbol_id) => self.symbol_shapes.get(&symbol_id),
                    None => self.library.globals.get(name)?.shape(),
                }
            }
            Expression::Var(Var::FieldAccess(_)) => {
                self.library.lookup(&self.global_path(expr)?)?.shape()
            }
            Expression::FunctionCall(call) => ensure_sufficient_stack(|| self.call_shape(call)),
            Expression::Parenthesized(parenthesized) => {
                ensure_sufficient_stack(|| self.expr_shape(&parenthesized.expr))
            }
            // Literal strings carry the derived `string` receiver shape, so
            // `("x"):upper()` and `local s = "x" ... s:upper()` resolve.
            Expression::StringLiteral(_) | Expression::InterpolatedString(_) => {
                string_shape(self.library)
            }
            Expression::Var(Var::Index(_))
            | Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_) => None,
        }
    }

    /// The function a colon call `receiver:method_name(...)` reaches through
    /// the receiver's shape, along with that shape.
    ///
    /// Only method-flagged functions are colon-callable; a dot member
    /// reached with `:` would receive a bogus self.
    pub(crate) fn method_callee(
        self,
        receiver: &Expression,
        method_name: &str,
    ) -> Option<(&'input CompactString, &'input StdlibEntry)> {
        let shape = self.expr_shape(receiver)?;
        let entry = self.library.shape_member(shape, method_name)?;
        let StdlibEntry::Function(function) = entry else {
            return None;
        };
        if !function.is_method {
            return None;
        }
        Some((shape, entry))
    }

    /// Unwind a `Name(.Name)*` chain into path segments, root first. Any
    /// non-name link (indexing, calls, parens) aborts, as does a local root.
    pub(crate) fn global_path(self, expr: &Expression) -> Option<Vec<&str>> {
        let Expression::Var(var) = expr else {
            return None;
        };
        let DottedPath { segments, root_span, .. } = dotted_path(var)?;
        if self.tree.resolves_to_local(segments.first()?, root_span) {
            return None;
        }
        Some(segments)
    }

    /// Shape of a call's first return value.
    ///
    /// Unlike `SemanticAnalysis::resolve_callee`, this accepts a method
    /// called with `.`: passing self explicitly doesn't change what the call
    /// returns.
    fn call_shape(self, call: &FunctionCall) -> Option<&'input CompactString> {
        let entry = match &call.method {
            Some(method_token) => self.method_callee(&call.callee, method_token.identifier()?)?.1,
            None => self.library.lookup(&self.global_path(&call.callee)?)?,
        };
        match entry {
            StdlibEntry::Function(function) => function.returns_shape.as_ref(),
            StdlibEntry::Constant(_) | StdlibEntry::Property(_) | StdlibEntry::Namespace(_) => None,
        }
    }
}

/// The derived `string` shape's key, when the library has one (it does
/// whenever a `string` namespace exists). Returned as the map key so
/// callers get a library-lifetime reference.
fn string_shape(library: &StdlibLibrary) -> Option<&CompactString> {
    library.shapes.get_key_value("string").map(|(name, _shape)| name)
}

/// Compute the shape of every never-reassigned local whose initializer has
/// a known shape, in `local` statements with one initializer per name.
///
/// The walk is in source order, so a shaped local can seed later ones.
pub(crate) fn symbol_shapes(
    tree: &ScopeTree,
    library: &StdlibLibrary,
    block: &Block,
) -> SymbolShapes {
    let span_to_symbol: FxHashMap<Span, SymbolId> =
        tree.symbols.iter().map(|symbol| (symbol.definition_span, symbol.id)).collect();
    let mut pass = ShapePass { tree, library, span_to_symbol, shapes: SymbolShapes::default() };
    pass.visit_block(block);
    pass.shapes
}

struct ShapePass<'input> {
    tree: &'input ScopeTree,
    library: &'input StdlibLibrary,
    span_to_symbol: FxHashMap<Span, SymbolId>,
    shapes: SymbolShapes,
}

impl<'ast> Visitor<'ast> for ShapePass<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        if let Statement::LocalAssignment(local) = stmt
            && let Some(exprs) = &local.exprs
            && local.names.len() == exprs.len()
        {
            for (attributed, expr) in local.names.iter().zip(exprs.iter()) {
                let resolver = ShapeResolver::new(self.tree, self.library, &self.shapes);
                let Some(shape) = resolver.expr_shape(expr).cloned() else {
                    continue;
                };
                let Some(&symbol_id) = self.span_to_symbol.get(&attributed.name.span) else {
                    continue;
                };
                if self.tree.symbol(symbol_id).is_modified {
                    continue;
                }
                self.shapes.insert(symbol_id, shape);
            }
        }
        self.walk_stmt(stmt);
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{LuaVersion, StdlibEnvironment};

    use super::*;
    use crate::builder;
    use crate::stdlib_load::parse_library;

    const SHAPED_LIBRARY: &str = r#"
        [globals.game]
        kind = "property"
        shape = "DataModel"

        [globals.io]
        kind = "namespace"
        [globals.io.members.open]
        kind = "function"
        min_args = 1
        max_args = 2
        params = [{ kind = "string" }, { kind = "string", is_required = false }]
        returns = "file"

        [shapes.DataModel.members.GetService]
        kind = "function"
        is_method = true
        min_args = 1
        max_args = 1
        params = [{ kind = "string" }]
        returns = "Instance"

        [shapes.Instance.members.FindFirstChild]
        kind = "function"
        is_method = true
        min_args = 1
        max_args = 2
        params = [{ kind = "string" }, { kind = "bool", is_required = false }]

        [shapes.file.members.read]
        kind = "function"
        is_method = true
        min_args = 0
        max_args = -1
        params = [{ kind = "vararg", is_required = false }]

        [globals.string]
        kind = "namespace"
        [globals.string.members.upper]
        kind = "function"
        min_args = 1
        max_args = 1
        is_must_use = true
        params = [{ kind = "string" }]
    "#;

    fn shaped_library() -> StdlibLibrary {
        parse_library(&[SHAPED_LIBRARY], LuaVersion::Lua54, StdlibEnvironment::Standalone)
            .expect("test catalog loads")
    }

    fn shapes_for(source: &str, library: &StdlibLibrary) -> (ScopeTree, SymbolShapes) {
        let parsed = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let tree = builder::build(&parsed.block);
        let shapes = symbol_shapes(&tree, library, &parsed.block);
        (tree, shapes)
    }

    fn shape_of<'shapes>(
        tree: &ScopeTree,
        shapes: &'shapes SymbolShapes,
        name: &str,
    ) -> Option<&'shapes str> {
        let symbol = tree.symbols.iter().find(|symbol| symbol.name == name)?;
        shapes.get(&symbol.id).map(CompactString::as_str)
    }

    #[test]
    fn local_from_shaped_call_gets_shape() {
        let library = shaped_library();
        let (tree, shapes) = shapes_for("local f = io.open('x')", &library);
        assert_eq!(shape_of(&tree, &shapes, "f"), Some("file"));
    }

    #[test]
    fn local_from_shaped_global_gets_shape() {
        let library = shaped_library();
        let (tree, shapes) = shapes_for("local g = game", &library);
        assert_eq!(shape_of(&tree, &shapes, "g"), Some("DataModel"));
    }

    #[test]
    fn method_chain_propagates_shapes() {
        let library = shaped_library();
        let (tree, shapes) = shapes_for(
            "local players = game:GetService('Players')\nlocal again = players:FindFirstChild('x')",
            &library,
        );
        assert_eq!(shape_of(&tree, &shapes, "players"), Some("Instance"));
        // FindFirstChild declares no return shape.
        assert_eq!(shape_of(&tree, &shapes, "again"), None);
    }

    #[test]
    fn reassigned_local_has_no_shape() {
        let library = shaped_library();
        let (tree, shapes) = shapes_for("local f = io.open('x')\nf = 1", &library);
        assert_eq!(shape_of(&tree, &shapes, "f"), None);
    }

    #[test]
    fn shadowed_global_base_has_no_shape() {
        let library = shaped_library();
        let (tree, shapes) = shapes_for("local game = 1\nlocal g = game", &library);
        assert_eq!(shape_of(&tree, &shapes, "g"), None);
    }

    #[test]
    fn string_literal_gets_string_shape() {
        let library = shaped_library();
        let (tree, shapes) = shapes_for("local s = 'abc'", &library);
        assert_eq!(shape_of(&tree, &shapes, "s"), Some("string"));
    }

    #[test]
    fn parenthesized_literal_method_chains_string_shape() {
        // upper is derived with returns = "string", so the call result
        // seeds the local's shape.
        let library = shaped_library();
        let (tree, shapes) = shapes_for("local u = ('x'):upper()", &library);
        assert_eq!(shape_of(&tree, &shapes, "u"), Some("string"));
    }

    #[test]
    fn misaligned_multi_assignment_ignored() {
        let library = shaped_library();
        let (tree, shapes) = shapes_for("local a, b = io.open('x')", &library);
        assert_eq!(shape_of(&tree, &shapes, "a"), None);
        assert_eq!(shape_of(&tree, &shapes, "b"), None);
    }

    #[test]
    fn method_called_with_dot_still_yields_return_shape() {
        let library = shaped_library();
        let (tree, shapes) =
            shapes_for("local players = game.GetService(game, 'Players')", &library);
        assert_eq!(shape_of(&tree, &shapes, "players"), Some("Instance"));
    }
}
