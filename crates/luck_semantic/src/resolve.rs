//! There's no type inference.
//!
//! A value has a known shape only when it's a
//! stdlib value that declares one, a string literal, a call whose entry
//! declares a return shape, or a never-reassigned local initialized from
//! one of those.
//!
//! A chain resolves from its prefix forward, a suffix at a time, so what it
//! reads, or what any of its calls calls, costs one pass over it.

use luck_ast::{ChainHead, Expression, Suffix, SuffixedExpression};
use luck_token::Token;
use rustc_hash::FxHashMap;

use crate::scope::{ScopeTree, SymbolId};
use crate::stdlib_model::{StdlibEntry, StdlibLibrary};

pub(crate) type SymbolShapes = FxHashMap<SymbolId, &'static str>;

/// What a chain's head reads, as far as it's known.
#[derive(Debug, Clone, Copy, Default)]
struct HeadState {
    shape: Option<&'static str>,
    /// The stdlib entry the head names while it's a dotted path from the
    /// host's global.
    global: Option<&'static StdlibEntry>,
}

impl HeadState {
    /// No suffix after an unknown head is known either.
    const fn is_unknown(self) -> bool {
        self.shape.is_none() && self.global.is_none()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ShapeResolver<'input> {
    tree: &'input ScopeTree,
    library: &'static StdlibLibrary,
    symbol_shapes: &'input SymbolShapes,
}

impl<'input> ShapeResolver<'input> {
    pub(crate) const fn new(
        tree: &'input ScopeTree,
        library: &'static StdlibLibrary,
        symbol_shapes: &'input SymbolShapes,
    ) -> Self {
        Self { tree, library, symbol_shapes }
    }

    /// The shape of `expr`, when statically known. See the module docs for
    /// what "known" means.
    pub(crate) fn expr_shape(self, expr: &Expression) -> Option<&'static str> {
        match expr {
            Expression::Name(token) => self.name_state(token).shape,
            Expression::Suffixed(_) => self.head_state(ChainHead::from(expr)).shape,
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

    /// What `token` reads: a local's shape, or the host's global and its
    /// shape.
    fn name_state(self, token: &Token) -> HeadState {
        let Some(reference) =
            token.identifier().and_then(|name| self.tree.reference_at(name, token.span))
        else {
            return HeadState::default();
        };
        match reference.resolved {
            Some(symbol_id) => {
                HeadState { shape: self.symbol_shapes.get(&symbol_id).copied(), global: None }
            }
            None if self.tree.is_host_global(reference) => {
                let global = self.library.globals.get(&reference.name);
                HeadState { shape: global.and_then(StdlibEntry::shape), global }
            }
            None => HeadState::default(),
        }
    }

    fn prefix_state(self, prefix: &Expression) -> HeadState {
        if let Expression::Name(token) = prefix {
            self.name_state(token)
        } else {
            HeadState { shape: self.expr_shape(prefix), global: None }
        }
    }

    fn head_state(self, head: ChainHead<'_>) -> HeadState {
        self.read(self.prefix_state(head.prefix), head.suffixes)
    }

    /// What a head that reads `state` reads once `suffixes` apply to it.
    fn read(self, mut state: HeadState, suffixes: &[Suffix]) -> HeadState {
        for suffix in suffixes {
            if state.is_unknown() {
                break;
            }
            state = self.after_suffix(suffix, state);
        }
        state
    }

    /// A call accepts a method called with `.`: passing self explicitly
    /// doesn't change what the call returns.
    fn after_suffix(self, suffix: &Suffix, state: HeadState) -> HeadState {
        match suffix {
            Suffix::Field(name) => {
                let member = state
                    .global
                    .zip(name.identifier())
                    .and_then(|(parent, segment)| self.library.child(parent, segment));
                HeadState { shape: member.and_then(StdlibEntry::shape), global: member }
            }
            Suffix::Call(call) => {
                let callee = call.method.as_ref().map_or(state.global, |method| {
                    self.shape_method(state.shape?, method.identifier()?)
                });
                let shape = match callee {
                    Some(StdlibEntry::Function(function)) => function.returns_shape,
                    Some(
                        StdlibEntry::Constant(_)
                        | StdlibEntry::Property(_)
                        | StdlibEntry::Namespace(_),
                    )
                    | None => None,
                };
                HeadState { shape, global: None }
            }
            Suffix::Index { .. } | Suffix::TypeInstantiation { .. } => HeadState::default(),
        }
    }

    /// Only method-flagged functions are colon-callable; a dot member
    /// reached with `:` would receive a bogus self.
    fn shape_method(self, shape: &str, method_name: &str) -> Option<&'static StdlibEntry> {
        let entry = self.library.shape_member(shape, method_name)?;
        matches!(entry, StdlibEntry::Function(function) if function.is_method).then_some(entry)
    }

    /// What a call with `method` after `head`, which reads `state`, calls.
    fn callee(
        self,
        head: ChainHead<'input>,
        state: HeadState,
        method: Option<&'input Token>,
    ) -> Option<ResolvedCallee<'input>> {
        let Some(method_token) = method else {
            let entry = state.global?;
            // A method called with `.` passes self explicitly, which its
            // declared arity doesn't count.
            if matches!(entry, StdlibEntry::Function(function) if function.is_method) {
                return None;
            }
            return Some(ResolvedCallee { path: CalleePath::Dotted(head), entry });
        };
        let method_name = method_token.identifier()?;
        let shape = state.shape?;
        let entry = self.shape_method(shape, method_name)?;
        let base = head.as_name().map_or(shape, |token| token.identifier().unwrap_or(shape));
        Some(ResolvedCallee { path: CalleePath::Method { base, method: method_name }, entry })
    }

    /// What a call with `method` after `head` calls, read from `head`'s
    /// prefix.
    pub(crate) fn head_callee(
        self,
        head: ChainHead<'input>,
        method: Option<&'input Token>,
    ) -> Option<ResolvedCallee<'input>> {
        self.callee(head, self.head_state(head), method)
    }
}

/// The derived `string` shape, when the library has one (it does whenever a
/// `string` namespace exists).
fn string_shape(library: &StdlibLibrary) -> Option<&'static str> {
    library.shapes.contains_key("string").then_some("string")
}

/// The shape of each never-reassigned local in `initialized_locals` whose
/// initializer has a known shape. A local comes before any initializer that
/// reads it, so a shaped local seeds later ones.
pub(crate) fn symbol_shapes(
    tree: &ScopeTree,
    library: &'static StdlibLibrary,
    initialized_locals: &[(SymbolId, &Expression)],
) -> SymbolShapes {
    let mut shapes = SymbolShapes::default();
    for &(symbol_id, initializer) in initialized_locals {
        if tree.symbol(symbol_id).is_modified {
            continue;
        }
        let resolver = ShapeResolver::new(tree, library, &shapes);
        let Some(shape) = resolver.expr_shape(initializer) else {
            continue;
        };
        shapes.insert(symbol_id, shape);
    }
    shapes
}

/// A call's stdlib entry, and the path that reaches it.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedCallee<'call> {
    path: CalleePath<'call>,
    entry: &'static StdlibEntry,
}

#[derive(Debug, Clone, Copy)]
enum CalleePath<'call> {
    Method {
        base: &'call str,
        method: &'call str,
    },
    /// A head whose every suffix is a field.
    Dotted(ChainHead<'call>),
}

impl ResolvedCallee<'_> {
    /// Display path, such as `table.insert` or `game:GetService`.
    #[must_use]
    pub fn name(&self) -> String {
        match self.path {
            CalleePath::Method { base, method } => format!("{base}:{method}"),
            CalleePath::Dotted(head) => {
                head.dotted_path().map(|path| path.segments.join(".")).unwrap_or_default()
            }
        }
    }

    #[must_use]
    pub const fn entry(&self) -> &'static StdlibEntry {
        self.entry
    }
}

/// Resolves the calls of one chain, carrying what each suffix reads on to
/// the next, so its calls resolve in one pass over it when asked for in
/// order.
#[derive(Debug, Clone, Copy)]
pub struct ChainCallees<'chain> {
    resolver: ShapeResolver<'chain>,
    chain: &'chain SuffixedExpression,
    prefix_state: HeadState,
    /// What the head of the first `read_count` suffixes reads.
    state: HeadState,
    read_count: usize,
}

impl<'chain> ChainCallees<'chain> {
    pub(crate) fn new(resolver: ShapeResolver<'chain>, chain: &'chain SuffixedExpression) -> Self {
        let prefix_state = resolver.prefix_state(&chain.prefix);
        Self { resolver, chain, prefix_state, state: prefix_state, read_count: 0 }
    }

    #[must_use]
    pub const fn chain(&self) -> &'chain SuffixedExpression {
        self.chain
    }

    /// What the call at `index` calls, as
    /// [`SemanticAnalysis::resolve_callee`](crate::SemanticAnalysis::resolve_callee)
    /// gives it. `None` for a suffix that's no call.
    ///
    /// A call is read on from the one asked for before it, so one earlier in
    /// the chain starts over from the prefix.
    pub fn callee(&mut self, index: usize) -> Option<ResolvedCallee<'chain>> {
        let suffixes: &'chain [Suffix] = &self.chain.suffixes;
        let Suffix::Call(call) = suffixes.get(index)? else {
            return None;
        };
        if index < self.read_count {
            self.state = self.prefix_state;
            self.read_count = 0;
        }
        let unread = suffixes.get(self.read_count..index).unwrap_or_default();
        self.state = self.resolver.read(self.state, unread);
        self.read_count = index;
        let head = ChainHead {
            prefix: &self.chain.prefix,
            suffixes: suffixes.get(..index).unwrap_or_default(),
        };
        self.resolver.callee(head, self.state, call.method.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{LuaVersion, StdlibEnvironment};

    use super::*;
    use crate::builder;
    use crate::stdlib_catalog::{FUNCTION, NAMESPACE, VALUE, optional, required, signature};
    use crate::stdlib_model::{
        StdlibArgKind, StdlibFunction, StdlibMap, StdlibNamespace, StdlibShape, StdlibValue,
    };

    static SHAPED_LIBRARY: StdlibLibrary = StdlibLibrary {
        version: LuaVersion::Lua54,
        environment: StdlibEnvironment::Standalone,
        globals: StdlibMap::new(&[
            ("game", StdlibEntry::Property(StdlibValue { shape: Some("DataModel"), ..VALUE })),
            (
                "io",
                StdlibEntry::Namespace(StdlibNamespace {
                    members: StdlibMap::new(&[(
                        "open",
                        StdlibEntry::Function(&StdlibFunction {
                            signatures: &[signature(
                                &[required(StdlibArgKind::String), optional(StdlibArgKind::String)],
                                1,
                                Some(2),
                            )],
                            returns_shape: Some("file"),
                            ..FUNCTION
                        }),
                    )]),
                    ..NAMESPACE
                }),
            ),
            (
                "string",
                StdlibEntry::Namespace(StdlibNamespace {
                    members: StdlibMap::new(&[(
                        "upper",
                        StdlibEntry::Function(&StdlibFunction {
                            signatures: &[signature(
                                &[required(StdlibArgKind::String)],
                                1,
                                Some(1),
                            )],
                            is_must_use: true,
                            ..FUNCTION
                        }),
                    )]),
                    ..NAMESPACE
                }),
            ),
        ]),
        shapes: StdlibMap::new(&[
            (
                "DataModel",
                StdlibShape {
                    members: StdlibMap::new(&[(
                        "GetService",
                        StdlibEntry::Function(&StdlibFunction {
                            signatures: &[signature(
                                &[required(StdlibArgKind::String)],
                                1,
                                Some(1),
                            )],
                            is_must_use: true,
                            is_method: true,
                            returns_shape: Some("Instance"),
                            ..FUNCTION
                        }),
                    )]),
                },
            ),
            (
                "Instance",
                StdlibShape {
                    members: StdlibMap::new(&[(
                        "FindFirstChild",
                        StdlibEntry::Function(&StdlibFunction {
                            signatures: &[signature(
                                &[required(StdlibArgKind::String), optional(StdlibArgKind::Bool)],
                                1,
                                Some(2),
                            )],
                            is_method: true,
                            ..FUNCTION
                        }),
                    )]),
                },
            ),
            (
                "file",
                StdlibShape {
                    members: StdlibMap::new(&[(
                        "read",
                        StdlibEntry::Function(&StdlibFunction {
                            signatures: &[signature(&[optional(StdlibArgKind::Vararg)], 0, None)],
                            is_method: true,
                            ..FUNCTION
                        }),
                    )]),
                },
            ),
            (
                "string",
                StdlibShape {
                    members: StdlibMap::new(&[(
                        "upper",
                        StdlibEntry::Function(&StdlibFunction {
                            signatures: &[signature(&[], 0, Some(0))],
                            is_must_use: true,
                            is_method: true,
                            returns_shape: Some("string"),
                            ..FUNCTION
                        }),
                    )]),
                },
            ),
        ]),
    };

    fn shapes_for(source: &str) -> (ScopeTree, SymbolShapes) {
        let parsed = luck_parser::parse(source, LuaVersion::Lua54);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let (tree, initialized_locals) =
            builder::build_with_initializers(&parsed.block, LuaVersion::Lua54);
        let shapes = symbol_shapes(&tree, &SHAPED_LIBRARY, &initialized_locals);
        (tree, shapes)
    }

    fn shape_of(tree: &ScopeTree, shapes: &SymbolShapes, name: &str) -> Option<&'static str> {
        let symbol = tree.symbols.iter().find(|symbol| symbol.name == name)?;
        shapes.get(&symbol.id).copied()
    }

    #[test]
    fn local_from_shaped_call_gets_shape() {
        let (tree, shapes) = shapes_for("local f = io.open('x')");
        assert_eq!(shape_of(&tree, &shapes, "f"), Some("file"));
    }

    #[test]
    fn local_from_shaped_global_gets_shape() {
        let (tree, shapes) = shapes_for("local g = game");
        assert_eq!(shape_of(&tree, &shapes, "g"), Some("DataModel"));
    }

    #[test]
    fn method_chain_propagates_shapes() {
        let (tree, shapes) = shapes_for(
            "local players = game:GetService('Players')\nlocal again = players:FindFirstChild('x')",
        );
        assert_eq!(shape_of(&tree, &shapes, "players"), Some("Instance"));
        // FindFirstChild declares no return shape.
        assert_eq!(shape_of(&tree, &shapes, "again"), None);
    }

    #[test]
    fn reassigned_local_has_no_shape() {
        let (tree, shapes) = shapes_for("local f = io.open('x')\nf = 1");
        assert_eq!(shape_of(&tree, &shapes, "f"), None);
    }

    #[test]
    fn shadowed_global_base_has_no_shape() {
        let (tree, shapes) = shapes_for("local game = 1\nlocal g = game");
        assert_eq!(shape_of(&tree, &shapes, "g"), None);
    }

    #[test]
    fn string_literal_gets_string_shape() {
        let (tree, shapes) = shapes_for("local s = 'abc'");
        assert_eq!(shape_of(&tree, &shapes, "s"), Some("string"));
    }

    #[test]
    fn parenthesized_literal_method_chains_string_shape() {
        // The string shape's upper returns a string, so the call result
        // seeds the local's shape.
        let (tree, shapes) = shapes_for("local u = ('x'):upper()");
        assert_eq!(shape_of(&tree, &shapes, "u"), Some("string"));
    }

    #[test]
    fn misaligned_multi_assignment_ignored() {
        let (tree, shapes) = shapes_for("local a, b = io.open('x')");
        assert_eq!(shape_of(&tree, &shapes, "a"), None);
        assert_eq!(shape_of(&tree, &shapes, "b"), None);
    }

    #[test]
    fn method_called_with_dot_still_yields_return_shape() {
        let (tree, shapes) = shapes_for("local players = game.GetService(game, 'Players')");
        assert_eq!(shape_of(&tree, &shapes, "players"), Some("Instance"));
    }
}
