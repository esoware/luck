//! Conservative stdlib resolution over expressions: maps call sites and
//! value reads to `StdlibEntry`s, including colon-method calls on shaped
//! values (`game:GetService(...)`, `f:read()` where `f = io.open(...)`).
//!
//! Resolution is deliberately shallow - no type inference. A value has a
//! known shape only when it flows directly: a global that declares a
//! shape, a call whose stdlib entry declares a return shape, or a local
//! initialized from one of those and never reassigned.

use std::collections::HashMap;

use compact_str::CompactString;
use luck_ast::expr::{Expression, FunctionCall, Var};
use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;
use luck_token::{Span, Token, TokenKind};

use crate::SemanticAnalysis;
use crate::scope::{ScopeTree, SymbolId};
use crate::stdlib_model::{StdlibEntry, StdlibLibrary};

/// A stdlib entry resolved from a call's callee.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedCallee<'lib> {
    pub entry: &'lib StdlibEntry,
    /// Whether the call used colon-method syntax.
    pub is_method_call: bool,
}

impl SemanticAnalysis {
    /// Resolve the callee of `call` to a stdlib entry plus a display
    /// path like `table.insert` or `game:GetService`. Returns `None`
    /// for shadowed bases, unknown names, and values whose shape is not
    /// statically known.
    #[must_use]
    pub fn resolve_callee(&self, call: &FunctionCall) -> Option<(String, ResolvedCallee<'_>)> {
        let lib = self.stdlib();
        if let Some(method_token) = &call.method {
            let method_name = identifier(method_token)?;
            let shape = self.expression_shape(&call.callee)?;
            let entry = lib.shape_member(shape, method_name)?;
            // Only method-flagged functions are colon-callable; a dot
            // member reached with `:` would receive a bogus self.
            let StdlibEntry::Function(func) = entry else {
                return None;
            };
            if !func.is_method {
                return None;
            }
            let base = match &call.callee {
                Expression::Var(Var::Name(token)) => identifier(token).unwrap_or(shape),
                _ => shape.as_str(),
            };
            return Some((
                format!("{base}:{method_name}"),
                ResolvedCallee {
                    entry,
                    is_method_call: true,
                },
            ));
        }
        let (segments, spans) = var_path(&call.callee)?;
        if is_local(&self.scope_tree, segments[0], spans[0]) {
            return None;
        }
        let entry = lib.lookup_str(&segments)?;
        // A method-flagged function reached with `.` takes an explicit
        // self, so its declared arity would be off by one - skip.
        if let StdlibEntry::Function(func) = entry
            && func.is_method
        {
            return None;
        }
        Some((
            segments.join("."),
            ResolvedCallee {
                entry,
                is_method_call: false,
            },
        ))
    }

    /// The shape of `expr`, when statically known. See module docs for
    /// what "known" means.
    #[must_use]
    pub fn expression_shape(&self, expr: &Expression) -> Option<&CompactString> {
        expression_shape(&self.scope_tree, self.stdlib(), &self.symbol_shapes, expr)
    }

    /// Resolve a dotted or colon path from its textual segments, where
    /// the root may be a shaped local: `["f", "read"]` with
    /// `local f = io.open(...)` resolves through the `file` shape, and
    /// a non-local root falls back to a plain library lookup. A local
    /// root without a known shape resolves to nothing, which doubles as
    /// the shadowing guard for path-based consumers (hover, tokens).
    #[must_use]
    pub fn resolve_stdlib_path(
        &self,
        segments: &[&str],
        root_span: Span,
    ) -> Option<&'static StdlibEntry> {
        let lib = self.stdlib();
        let first = segments.first()?;
        let resolved = self
            .scope_tree
            .references
            .iter()
            .find(|reference| reference.span == root_span && reference.name == *first)
            .and_then(|reference| reference.resolved);
        match resolved {
            Some(symbol_id) => {
                let shape = self.symbol_shapes.get(&symbol_id)?;
                let mut current = lib.shape_member(shape, segments.get(1)?)?;
                for segment in &segments[2..] {
                    current = lib.child(current, segment)?;
                }
                Some(current)
            }
            None => lib.lookup_str(segments),
        }
    }

    /// Best-effort lexical lookup for completion: the shape of the
    /// nearest local named `name` declared before `offset`. Mid-typing
    /// text like `f:` has no parsed reference to resolve span-exactly,
    /// so this mirrors the lexical discipline of the completion
    /// provider's visible-locals walk.
    #[must_use]
    pub fn shape_of_nearest_local(&self, name: &str, offset: u32) -> Option<&CompactString> {
        let symbol = self
            .scope_tree
            .symbols
            .iter()
            .filter(|symbol| symbol.name == name && symbol.definition_span.end <= offset)
            .max_by_key(|symbol| symbol.definition_span.start)?;
        self.symbol_shapes.get(&symbol.id)
    }
}

fn identifier(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Identifier(name) => Some(name.as_str()),
        _ => None,
    }
}

/// Unwind a `Name(.Name)*` chain into path segments and their spans.
/// Any non-name link (indexing, calls, parens) aborts.
fn var_path(expr: &Expression) -> Option<(Vec<&str>, Vec<Span>)> {
    let mut segments: Vec<&str> = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut cursor = expr;
    loop {
        match cursor {
            Expression::Var(Var::Name(token)) => {
                segments.push(identifier(token)?);
                spans.push(token.span);
                break;
            }
            Expression::Var(Var::FieldAccess(field_access)) => {
                segments.push(identifier(&field_access.name)?);
                spans.push(field_access.name.span);
                cursor = &field_access.prefix;
            }
            _ => return None,
        }
    }
    segments.reverse();
    spans.reverse();
    Some((segments, spans))
}

/// Span-exact "is this name a local here" check, mirroring
/// `SemanticAnalysis::resolves_to_local`.
fn is_local(tree: &ScopeTree, name: &str, span: Span) -> bool {
    tree.references.iter().any(|reference| {
        reference.span == span && reference.name == name && reference.resolved.is_some()
    })
}

fn expression_shape<'out>(
    tree: &ScopeTree,
    lib: &'out StdlibLibrary,
    symbol_shapes: &'out HashMap<SymbolId, CompactString>,
    expr: &Expression,
) -> Option<&'out CompactString> {
    match expr {
        Expression::Var(Var::Name(token)) => {
            let name = identifier(token)?;
            let resolved = tree
                .references
                .iter()
                .find(|reference| reference.span == token.span && reference.name == name)
                .and_then(|reference| reference.resolved);
            match resolved {
                Some(symbol_id) => symbol_shapes.get(&symbol_id),
                None => value_shape(lib.globals.get(name)?),
            }
        }
        Expression::Var(Var::FieldAccess(_)) => {
            let (segments, spans) = var_path(expr)?;
            if is_local(tree, segments[0], spans[0]) {
                return None;
            }
            value_shape(lib.lookup_str(&segments)?)
        }
        Expression::FunctionCall(call) => call_shape(tree, lib, symbol_shapes, call),
        Expression::Parenthesized(paren) => expression_shape(tree, lib, symbol_shapes, &paren.expr),
        // Literal strings carry the derived `string` receiver shape, so
        // ("x"):upper() and `local s = "x" ... s:upper()` resolve.
        Expression::StringLiteral(_) | Expression::InterpolatedString(_) => string_shape(lib),
        _ => None,
    }
}

/// The derived `string` shape's key, when the library has one (it does
/// whenever a `string` namespace exists). Returned as the map key so
/// callers get a library-lifetime reference.
fn string_shape(lib: &StdlibLibrary) -> Option<&CompactString> {
    lib.shapes
        .get_key_value("string")
        .map(|(name, _shape)| name)
}

fn value_shape(entry: &StdlibEntry) -> Option<&CompactString> {
    match entry {
        StdlibEntry::Constant(value) | StdlibEntry::Property(value) => value.shape.as_ref(),
        StdlibEntry::Namespace(namespace) => namespace.shape.as_ref(),
        StdlibEntry::Function(_) => None,
    }
}

/// Shape of a call's first return value. Duplicates the callee logic of
/// `resolve_callee` on free-function form so the shape-inference pass
/// can run before a `SemanticAnalysis` exists.
fn call_shape<'out>(
    tree: &ScopeTree,
    lib: &'out StdlibLibrary,
    symbol_shapes: &'out HashMap<SymbolId, CompactString>,
    call: &FunctionCall,
) -> Option<&'out CompactString> {
    let entry = if let Some(method_token) = &call.method {
        let method_name = identifier(method_token)?;
        let shape = expression_shape(tree, lib, symbol_shapes, &call.callee)?;
        let entry = lib.shape_member(shape, method_name)?;
        let StdlibEntry::Function(func) = entry else {
            return None;
        };
        if !func.is_method {
            return None;
        }
        entry
    } else {
        let (segments, spans) = var_path(&call.callee)?;
        if is_local(tree, segments[0], spans[0]) {
            return None;
        }
        lib.lookup_str(&segments)?
    };
    match entry {
        StdlibEntry::Function(func) => func.returns_shape.as_ref(),
        _ => None,
    }
}

/// Compute the shape of every eligible local: declared with a 1:1
/// name/initializer pairing, initializer of known shape, and never
/// reassigned. Source-order walk, so shaped locals can seed later ones.
pub(crate) fn compute_symbol_shapes(
    tree: &ScopeTree,
    lib: &StdlibLibrary,
    block: &Block,
) -> HashMap<SymbolId, CompactString> {
    let mut span_to_symbol: HashMap<Span, SymbolId> = HashMap::new();
    for symbol in &tree.symbols {
        span_to_symbol.insert(symbol.definition_span, symbol.id);
    }
    let mut pass = ShapePass {
        tree,
        lib,
        span_to_symbol,
        shapes: HashMap::new(),
    };
    pass.visit_block(block);
    pass.shapes
}

struct ShapePass<'a> {
    tree: &'a ScopeTree,
    lib: &'a StdlibLibrary,
    span_to_symbol: HashMap<Span, SymbolId>,
    shapes: HashMap<SymbolId, CompactString>,
}

impl<'ast> Visitor<'ast> for ShapePass<'_> {
    fn visit_statement(&mut self, stmt: &'ast luck_ast::Statement) {
        if let luck_ast::Statement::LocalAssignment(local) = stmt
            && let Some(exprs) = &local.exprs
            && local.names.len() == exprs.len()
        {
            for (attributed, expr) in local.names.iter().zip(exprs.iter()) {
                let Some(shape) =
                    expression_shape(self.tree, self.lib, &self.shapes, expr).cloned()
                else {
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
        self.walk_statement(stmt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::ScopeTreeBuilder;
    use crate::stdlib_model::parse_library;
    use luck_token::{LuaVersion, StdlibEnvironment};

    const SHAPED_LIB: &str = r#"
        [globals.game]
        kind = "property"
        shape = "DataModel"

        [globals.io]
        kind = "namespace"
        [globals.io.members.open]
        kind = "function"
        min_args = 1
        max_args = 2
        params = [{ kind = "string" }, { kind = "string", required = false }]
        returns = "file"

        [shapes.DataModel.members.GetService]
        kind = "function"
        method = true
        min_args = 1
        max_args = 1
        params = [{ kind = "string" }]
        returns = "Instance"

        [shapes.Instance.members.FindFirstChild]
        kind = "function"
        method = true
        min_args = 1
        max_args = 2
        params = [{ kind = "string" }, { kind = "bool", required = false }]

        [shapes.file.members.read]
        kind = "function"
        method = true
        min_args = 0
        max_args = -1
        params = [{ kind = "vararg", required = false }]

        [globals.string]
        kind = "namespace"
        [globals.string.members.upper]
        kind = "function"
        min_args = 1
        max_args = 1
        must_use = true
        params = [{ kind = "string" }]
    "#;

    fn shaped_lib() -> StdlibLibrary {
        parse_library(
            &[SHAPED_LIB],
            LuaVersion::Lua54,
            StdlibEnvironment::Standalone,
        )
    }

    fn shapes_for(
        source: &str,
        lib: &StdlibLibrary,
    ) -> (ScopeTree, HashMap<SymbolId, CompactString>) {
        let parsed = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let tree = ScopeTreeBuilder::new().build(&parsed.block);
        let shapes = compute_symbol_shapes(&tree, lib, &parsed.block);
        (tree, shapes)
    }

    fn shape_of<'a>(
        tree: &ScopeTree,
        shapes: &'a HashMap<SymbolId, CompactString>,
        name: &str,
    ) -> Option<&'a str> {
        let symbol = tree.symbols.iter().find(|symbol| symbol.name == name)?;
        shapes.get(&symbol.id).map(CompactString::as_str)
    }

    #[test]
    fn local_from_shaped_call_gets_shape() {
        let lib = shaped_lib();
        let (tree, shapes) = shapes_for("local f = io.open('x')", &lib);
        assert_eq!(shape_of(&tree, &shapes, "f"), Some("file"));
    }

    #[test]
    fn local_from_shaped_global_gets_shape() {
        let lib = shaped_lib();
        let (tree, shapes) = shapes_for("local g = game", &lib);
        assert_eq!(shape_of(&tree, &shapes, "g"), Some("DataModel"));
    }

    #[test]
    fn method_chain_propagates_shapes() {
        let lib = shaped_lib();
        let (tree, shapes) = shapes_for(
            "local players = game:GetService('Players')\nlocal again = players:FindFirstChild('x')",
            &lib,
        );
        assert_eq!(shape_of(&tree, &shapes, "players"), Some("Instance"));
        // FindFirstChild declares no return shape.
        assert_eq!(shape_of(&tree, &shapes, "again"), None);
    }

    #[test]
    fn reassigned_local_has_no_shape() {
        let lib = shaped_lib();
        let (tree, shapes) = shapes_for("local f = io.open('x')\nf = 1", &lib);
        assert_eq!(shape_of(&tree, &shapes, "f"), None);
    }

    #[test]
    fn shadowed_global_base_has_no_shape() {
        let lib = shaped_lib();
        let (tree, shapes) = shapes_for("local game = 1\nlocal g = game", &lib);
        assert_eq!(shape_of(&tree, &shapes, "g"), None);
    }

    #[test]
    fn string_literal_gets_string_shape() {
        let lib = shaped_lib();
        let (tree, shapes) = shapes_for("local s = 'abc'", &lib);
        assert_eq!(shape_of(&tree, &shapes, "s"), Some("string"));
    }

    #[test]
    fn parenthesized_literal_method_chains_string_shape() {
        // upper is derived with returns = "string", so the call result
        // seeds the local's shape.
        let lib = shaped_lib();
        let (tree, shapes) = shapes_for("local u = ('x'):upper()", &lib);
        assert_eq!(shape_of(&tree, &shapes, "u"), Some("string"));
    }

    #[test]
    fn misaligned_multi_assignment_ignored() {
        let lib = shaped_lib();
        let (tree, shapes) = shapes_for("local a, b = io.open('x')", &lib);
        assert_eq!(shape_of(&tree, &shapes, "a"), None);
        assert_eq!(shape_of(&tree, &shapes, "b"), None);
    }
}
