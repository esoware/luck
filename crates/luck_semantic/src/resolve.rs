//! There's no type inference.
//!
//! A value has a known shape only when it's a
//! stdlib value that declares one, a string literal, a call whose entry
//! declares a return shape, or a never-reassigned local initialized from
//! one of those.

use luck_ast::{
    Block, ChainHead, DottedPath, Expression, Statement, Suffix, SuffixedExpression, Visitor,
};
use luck_token::{CompactString, Span, Token};
use rustc_hash::FxHashMap;

use crate::scope::{ScopeTree, SymbolId};
use crate::stdlib_model::{StdlibEntry, StdlibLibrary};

pub(crate) type SymbolShapes = FxHashMap<SymbolId, CompactString>;

/// The shape of what a chain reads up to and including a suffix, keyed by
/// where the suffix starts. Only known shapes are kept.
pub(crate) type SuffixShapes = FxHashMap<u32, CompactString>;

#[derive(Clone, Copy)]
pub(crate) struct ShapeResolver<'input> {
    tree: &'input ScopeTree,
    library: &'input StdlibLibrary,
    symbol_shapes: &'input SymbolShapes,
    suffix_shapes: &'input SuffixShapes,
}

impl<'input> ShapeResolver<'input> {
    pub(crate) const fn new(
        tree: &'input ScopeTree,
        library: &'input StdlibLibrary,
        symbol_shapes: &'input SymbolShapes,
        suffix_shapes: &'input SuffixShapes,
    ) -> Self {
        Self { tree, library, symbol_shapes, suffix_shapes }
    }

    /// The shape of `expr`, when statically known. See the module docs for
    /// what "known" means.
    pub(crate) fn expr_shape(self, expr: &Expression) -> Option<&'input CompactString> {
        match expr {
            Expression::Name(token) => self.name_shape_and_global(token).0,
            Expression::Suffixed(chain) => {
                self.suffix_shapes.get(&chain.suffixes.last().span().start)
            }
            Expression::Parenthesized(parenthesized) => self.expr_shape(&parenthesized.expr),
            // Literal strings carry the derived `string` receiver shape, so
            // `("x"):upper()` and `local s = "x" ... s:upper()` resolve.
            Expression::StringLiteral(_) | Expression::InterpolatedString(_) => {
                string_shape(self.library)
            }
            Expression::Nil(_)
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
            | Expression::Error(_) => None,
        }
    }

    /// The shape of what `token` names, and the stdlib global it reads, if
    /// it reads the host's.
    fn name_shape_and_global(
        self,
        token: &Token,
    ) -> (Option<&'input CompactString>, Option<&'input StdlibEntry>) {
        let Some(name) = token.identifier() else {
            return (None, None);
        };
        match self.tree.resolved_symbol(name, token.span) {
            Some(symbol_id) => (self.symbol_shapes.get(&symbol_id), None),
            None if self.tree.reaches_host_global(name, token.span) => {
                let global = self.library.globals.get(name);
                (global.and_then(StdlibEntry::shape), global)
            }
            None => (None, None),
        }
    }

    fn head_shape(self, head: ChainHead<'_>) -> Option<&'input CompactString> {
        head.suffixes.last().map_or_else(
            || self.expr_shape(head.prefix),
            |last| self.suffix_shapes.get(&last.span().start),
        )
    }

    pub(crate) fn method_callee(
        self,
        receiver: ChainHead<'_>,
        method_name: &str,
    ) -> Option<(&'input CompactString, &'input StdlibEntry)> {
        self.shape_method(self.head_shape(receiver)?, method_name)
    }

    /// Only method-flagged functions are colon-callable; a dot member
    /// reached with `:` would receive a bogus self.
    fn shape_method<'shape>(
        self,
        shape: &'shape CompactString,
        method_name: &str,
    ) -> Option<(&'shape CompactString, &'input StdlibEntry)> {
        let entry = self.library.shape_member(shape, method_name)?;
        let StdlibEntry::Function(function) = entry else {
            return None;
        };
        if !function.is_method {
            return None;
        }
        Some((shape, entry))
    }

    /// The path of a `Name(.Name)*` chain, root first. Any other link
    /// (indexing, calls, parens) aborts, as does a root that doesn't reach
    /// the host's global.
    pub(crate) fn global_path(self, head: ChainHead<'_>) -> Option<Vec<&str>> {
        let DottedPath { segments, root_span, .. } = head.dotted_path()?;
        self.tree.reaches_host_global(segments.first()?, root_span).then_some(segments)
    }

    /// The shape of a chain's head once `suffix` applies, and the stdlib
    /// entry it names when it's still a dotted path from the host's global,
    /// given the same for the head before it. A call accepts a method called
    /// with `.`: passing self explicitly doesn't change what the call
    /// returns.
    fn after_suffix(
        self,
        suffix: &Suffix,
        shape: Option<&CompactString>,
        global: Option<&'input StdlibEntry>,
    ) -> (Option<&'input CompactString>, Option<&'input StdlibEntry>) {
        match suffix {
            Suffix::Field(name) => {
                let member = global
                    .zip(name.identifier())
                    .and_then(|(parent, segment)| self.library.child(parent, segment));
                (member.and_then(StdlibEntry::shape), member)
            }
            Suffix::Call(call) => {
                let callee = call.method.as_ref().map_or(global, |method| {
                    shape
                        .zip(method.identifier())
                        .and_then(|(receiver, name)| self.shape_method(receiver, name))
                        .map(|(_, entry)| entry)
                });
                let returns = match callee {
                    Some(StdlibEntry::Function(function)) => function.returns_shape.as_ref(),
                    Some(
                        StdlibEntry::Constant(_)
                        | StdlibEntry::Property(_)
                        | StdlibEntry::Namespace(_),
                    )
                    | None => None,
                };
                (returns, None)
            }
            Suffix::Index { .. } | Suffix::TypeInstantiation { .. } => (None, None),
        }
    }
}

/// The derived `string` shape's key, when the library has one (it does
/// whenever a `string` namespace exists). Returned as the map key so
/// callers get a library-lifetime reference.
fn string_shape(library: &StdlibLibrary) -> Option<&CompactString> {
    library.shapes.get_key_value("string").map(|(name, _shape)| name)
}

/// The shape of every never-reassigned local whose initializer has a known
/// shape, in `local` statements with one initializer per name, and of each
/// chain up to each of its suffixes.
///
/// The walk is in source order, so a shaped local can seed later ones, and
/// a chain is done before any chain around it.
pub(crate) fn shapes(
    tree: &ScopeTree,
    library: &StdlibLibrary,
    block: &Block,
) -> (SymbolShapes, SuffixShapes) {
    let span_to_symbol: FxHashMap<Span, SymbolId> =
        tree.symbols.iter().map(|symbol| (symbol.definition_span, symbol.id)).collect();
    let mut pass = ShapePass {
        tree,
        library,
        span_to_symbol,
        symbol_shapes: SymbolShapes::default(),
        suffix_shapes: SuffixShapes::default(),
    };
    pass.visit_block(block);
    (pass.symbol_shapes, pass.suffix_shapes)
}

struct ShapePass<'input> {
    tree: &'input ScopeTree,
    library: &'input StdlibLibrary,
    span_to_symbol: FxHashMap<Span, SymbolId>,
    symbol_shapes: SymbolShapes,
    suffix_shapes: SuffixShapes,
}

impl ShapePass<'_> {
    const fn resolver(&self) -> ShapeResolver<'_> {
        ShapeResolver::new(self.tree, self.library, &self.symbol_shapes, &self.suffix_shapes)
    }

    fn record_chain(&mut self, chain: &SuffixedExpression) {
        let resolver = self.resolver();
        let (mut shape, mut global) = if let Expression::Name(token) = &chain.prefix {
            resolver.name_shape_and_global(token)
        } else {
            (resolver.expr_shape(&chain.prefix), None)
        };
        let mut known = Vec::new();
        for suffix in &chain.suffixes {
            // Once neither is known, no later suffix can be.
            if shape.is_none() && global.is_none() {
                break;
            }
            (shape, global) = resolver.after_suffix(suffix, shape, global);
            if let Some(found) = shape {
                known.push((suffix.span().start, found.clone()));
            }
        }
        self.suffix_shapes.extend(known);
    }
}

impl<'ast> Visitor<'ast> for ShapePass<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        self.walk_stmt(stmt);
        if let Statement::LocalAssignment(local) = stmt
            && let Some(exprs) = &local.exprs
            && local.names.len() == exprs.len()
        {
            for (attributed, expr) in local.names.iter().zip(exprs.iter()) {
                let Some(shape) = self.resolver().expr_shape(expr).cloned() else {
                    continue;
                };
                let Some(&symbol_id) = self.span_to_symbol.get(&attributed.name.span) else {
                    continue;
                };
                if self.tree.symbol(symbol_id).is_modified {
                    continue;
                }
                self.symbol_shapes.insert(symbol_id, shape);
            }
        }
    }

    fn visit_suffixed(&mut self, chain: &'ast SuffixedExpression) {
        self.walk_suffixed(chain);
        self.record_chain(chain);
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
        let tree = builder::build(&parsed.block, LuaVersion::Lua54);
        let (symbol_shapes, _) = shapes(&tree, library, &parsed.block);
        (tree, symbol_shapes)
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
