//! There's no type inference.
//!
//! A value has a known shape only when it's a
//! stdlib value that declares one, a string literal, a call whose entry
//! declares a return shape, or a never-reassigned local initialized from
//! one of those.
//!
//! A chain resolves from its prefix forward, a suffix at a time, so what it
//! reads, or what any of its calls calls, costs one pass over it.

use luck_ast::{Call, ChainHead, Expression, Name, Suffix, SuffixedExpression};

use crate::scope::{ScopeTree, SymbolId};
use crate::stdlib_model::{StdlibEntry, StdlibLibrary};

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
    tree: &'input ScopeTree<'input>,
    library: &'static StdlibLibrary,
    /// Each symbol's shape, by symbol index.
    symbol_shapes: &'input [Option<&'static str>],
}

impl<'input> ShapeResolver<'input> {
    pub(crate) const fn new(
        tree: &'input ScopeTree<'input>,
        library: &'static StdlibLibrary,
        symbol_shapes: &'input [Option<&'static str>],
    ) -> Self {
        Self { tree, library, symbol_shapes }
    }

    pub(crate) fn symbol_shape(self, symbol: SymbolId) -> Option<&'static str> {
        self.symbol_shapes.get(symbol.index()).copied().flatten()
    }

    /// The shape of `expr`, when statically known. See the module docs for
    /// what "known" means.
    fn expr_shape(self, expr: &Expression<'_>) -> Option<&'static str> {
        match expr {
            Expression::Name(name) => self.name_state(name).shape,
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

    /// What `name` reads: a local's shape, or the host's global and its
    /// shape.
    fn name_state(self, name: &Name<'_>) -> HeadState {
        let Some(reference) = self.tree.reference_of(name).map(|id| self.tree.reference(id)) else {
            return HeadState::default();
        };
        match reference.resolved {
            Some(symbol) => HeadState { shape: self.symbol_shape(symbol), global: None },
            None if self.tree.is_host_global(reference) => {
                let global = self.library.globals.get(name.text);
                HeadState { shape: global.and_then(StdlibEntry::shape), global }
            }
            None => HeadState::default(),
        }
    }

    fn prefix_state(self, prefix: &Expression<'_>) -> HeadState {
        if let Expression::Name(name) = prefix {
            self.name_state(name)
        } else {
            HeadState { shape: self.expr_shape(prefix), global: None }
        }
    }

    fn head_state(self, head: ChainHead<'_>) -> HeadState {
        self.read(self.prefix_state(head.prefix), head.suffixes)
    }

    /// What a head that reads `state` reads once `suffixes` apply to it.
    fn read(self, mut state: HeadState, suffixes: &[Suffix<'_>]) -> HeadState {
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
    fn after_suffix(self, suffix: &Suffix<'_>, state: HeadState) -> HeadState {
        let callee = match suffix {
            Suffix::Field(name) => {
                let member = state.global.and_then(|parent| self.library.child(parent, name.text));
                return HeadState { shape: member.and_then(StdlibEntry::shape), global: member };
            }
            Suffix::Call(_) => state.global,
            Suffix::MethodCall(call) => {
                state.shape.and_then(|shape| self.shape_method(shape, call.name.text))
            }
            Suffix::Index { .. } | Suffix::TypeInstantiation { .. } => None,
        };
        let shape = match callee {
            Some(StdlibEntry::Function(function)) => function.returns_shape,
            Some(
                StdlibEntry::Constant(_) | StdlibEntry::Property(_) | StdlibEntry::Namespace(_),
            )
            | None => None,
        };
        HeadState { shape, global: None }
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
        method: Option<&'input Name<'input>>,
    ) -> Option<ResolvedCallee<'input>> {
        let Some(&Name { text: method_name, .. }) = method else {
            let entry = state.global?;
            // A method called with `.` passes self explicitly, which its
            // declared arity doesn't count.
            if matches!(entry, StdlibEntry::Function(function) if function.is_method) {
                return None;
            }
            return Some(ResolvedCallee { path: CalleePath::Dotted(head), entry });
        };
        let shape = state.shape?;
        let entry = self.shape_method(shape, method_name)?;
        let base = head.as_name().map_or(shape, |name| name.text);
        Some(ResolvedCallee { path: CalleePath::Method { base, method: method_name }, entry })
    }

    /// What `call` calls, read from its callee's prefix.
    pub(crate) fn call_callee(self, call: Call<'input>) -> Option<ResolvedCallee<'input>> {
        self.callee(call.callee, self.head_state(call.callee), call.method)
    }
}

/// The derived `string` shape, when the library has one (it does whenever a
/// `string` namespace exists).
fn string_shape(library: &StdlibLibrary) -> Option<&'static str> {
    library.shapes.contains_key("string").then_some("string")
}

/// Each symbol's shape, by symbol index.
///
/// Only a never-reassigned local in `initialized_locals` whose initializer
/// has a known shape has one. A local comes before any initializer that
/// reads it, so a shaped local seeds later ones.
pub(crate) fn symbol_shapes(
    tree: &ScopeTree<'_>,
    library: &'static StdlibLibrary,
    initialized_locals: &[(SymbolId, &Expression<'_>)],
) -> Vec<Option<&'static str>> {
    let mut shapes = vec![None; tree.symbols().len()];
    for &(symbol, initializer) in initialized_locals {
        if tree.symbol(symbol).is_modified {
            continue;
        }
        let shape = ShapeResolver::new(tree, library, &shapes).expr_shape(initializer);
        if let Some(slot) = shapes.get_mut(symbol.index()) {
            *slot = shape;
        }
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

/// Resolves the calls of one chain front to back, carrying what each suffix
/// reads on to the next, so its calls resolve in one pass over it.
#[derive(Debug, Clone, Copy)]
pub struct ChainCallees<'chain> {
    resolver: ShapeResolver<'chain>,
    chain: &'chain SuffixedExpression<'chain>,
    /// What the head of the first `read_count` suffixes reads.
    state: HeadState,
    read_count: usize,
}

impl<'chain> ChainCallees<'chain> {
    pub(crate) fn new(
        resolver: ShapeResolver<'chain>,
        chain: &'chain SuffixedExpression<'chain>,
    ) -> Self {
        let state = resolver.prefix_state(&chain.prefix);
        Self { resolver, chain, state, read_count: 0 }
    }

    #[must_use]
    pub const fn chain(&self) -> &'chain SuffixedExpression<'chain> {
        self.chain
    }

    /// What the call at `index` calls, as
    /// [`SemanticAnalysis::resolve_callee`](crate::SemanticAnalysis::resolve_callee)
    /// gives it. `None` for a suffix that's no call.
    ///
    /// A call is read on from the one asked for before it, so `index` is
    /// never below an earlier one.
    pub fn callee(&mut self, index: usize) -> Option<ResolvedCallee<'chain>> {
        let suffixes: &'chain [Suffix<'chain>] = &self.chain.suffixes;
        let call = Call::new(self.chain.head(index), suffixes.get(index)?)?;
        let unread = suffixes.get(self.read_count..index).unwrap_or_default();
        self.state = self.resolver.read(self.state, unread);
        self.read_count = index;
        self.resolver.callee(call.callee, self.state, call.method)
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_token::{LuaVersion, StdlibEnvironment};

    use super::*;
    use crate::fixtures::parse;
    use crate::scope::build_with_initializers;
    use crate::stdlib_catalog::{FUNCTION, VALUE, optional, required, signature};
    use crate::stdlib_model::{
        StdlibArgKind, StdlibFunction, StdlibNamespace, StdlibShape, StdlibValue, stdlib_map,
    };

    static SHAPED_LIBRARY: StdlibLibrary = StdlibLibrary {
        version: LuaVersion::Lua54,
        environment: StdlibEnvironment::Standalone,
        globals: stdlib_map!(
            StdlibEntry,
            [
                ("game", StdlibEntry::Property(&StdlibValue { shape: Some("DataModel"), ..VALUE })),
                (
                    "io",
                    StdlibEntry::Namespace(&StdlibNamespace {
                        members: stdlib_map!(
                            StdlibEntry,
                            [(
                                "open",
                                StdlibEntry::Function(&StdlibFunction {
                                    signatures: &[signature(
                                        &[
                                            required(StdlibArgKind::String),
                                            optional(StdlibArgKind::String)
                                        ],
                                        1,
                                        Some(2),
                                    )],
                                    returns_shape: Some("file"),
                                    ..FUNCTION
                                }),
                            )]
                        ),
                        shape: None,
                        deprecated: None,
                    }),
                ),
                (
                    "string",
                    StdlibEntry::Namespace(&StdlibNamespace {
                        members: stdlib_map!(
                            StdlibEntry,
                            [(
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
                            )]
                        ),
                        shape: None,
                        deprecated: None,
                    }),
                ),
            ]
        ),
        shapes: stdlib_map!(
            StdlibShape,
            [
                (
                    "DataModel",
                    StdlibShape {
                        members: stdlib_map!(
                            StdlibEntry,
                            [(
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
                            )]
                        ),
                    },
                ),
                (
                    "Instance",
                    StdlibShape {
                        members: stdlib_map!(
                            StdlibEntry,
                            [(
                                "FindFirstChild",
                                StdlibEntry::Function(&StdlibFunction {
                                    signatures: &[signature(
                                        &[
                                            required(StdlibArgKind::String),
                                            optional(StdlibArgKind::Bool)
                                        ],
                                        1,
                                        Some(2),
                                    )],
                                    is_method: true,
                                    ..FUNCTION
                                }),
                            )]
                        ),
                    },
                ),
                (
                    "file",
                    StdlibShape {
                        members: stdlib_map!(
                            StdlibEntry,
                            [(
                                "read",
                                StdlibEntry::Function(&StdlibFunction {
                                    signatures: &[signature(
                                        &[optional(StdlibArgKind::Vararg)],
                                        0,
                                        None
                                    )],
                                    is_method: true,
                                    ..FUNCTION
                                }),
                            )]
                        ),
                    },
                ),
                (
                    "string",
                    StdlibShape {
                        members: stdlib_map!(
                            StdlibEntry,
                            [(
                                "upper",
                                StdlibEntry::Function(&StdlibFunction {
                                    signatures: &[signature(&[], 0, Some(0))],
                                    is_must_use: true,
                                    is_method: true,
                                    returns_shape: Some("string"),
                                    ..FUNCTION
                                }),
                            )]
                        ),
                    },
                ),
            ]
        ),
    };

    fn shape_of(source: &str, name: &str) -> Option<&'static str> {
        let allocator = Allocator::new();
        let block = parse(&allocator, source, LuaVersion::Lua54);
        let (tree, initialized_locals) = build_with_initializers(block, LuaVersion::Lua54, &mut ());
        let shapes = symbol_shapes(&tree, &SHAPED_LIBRARY, &initialized_locals);
        let symbol = tree.symbols().iter().find(|symbol| tree.name(symbol.name) == name)?;
        shapes.get(symbol.id.index()).copied().flatten()
    }

    #[test]
    fn local_from_shaped_call_gets_shape() {
        assert_eq!(shape_of("local f = io.open('x')", "f"), Some("file"));
    }

    #[test]
    fn local_from_shaped_global_gets_shape() {
        assert_eq!(shape_of("local g = game", "g"), Some("DataModel"));
    }

    #[test]
    fn method_chain_propagates_shapes() {
        let source = "\
local players = game:GetService('Players')
local again = players:FindFirstChild('x')";
        assert_eq!(shape_of(source, "players"), Some("Instance"));
        // FindFirstChild declares no return shape.
        assert_eq!(shape_of(source, "again"), None);
    }

    #[test]
    fn reassigned_local_has_no_shape() {
        let source = "\
local f = io.open('x')
f = 1";
        assert_eq!(shape_of(source, "f"), None);
    }

    #[test]
    fn shadowed_global_base_has_no_shape() {
        let source = "\
local game = 1
local g = game";
        assert_eq!(shape_of(source, "g"), None);
    }

    #[test]
    fn string_literal_gets_string_shape() {
        assert_eq!(shape_of("local s = 'abc'", "s"), Some("string"));
    }

    #[test]
    fn parenthesized_literal_method_chains_string_shape() {
        // The string shape's upper returns a string, so the call result
        // seeds the local's shape.
        assert_eq!(shape_of("local u = ('x'):upper()", "u"), Some("string"));
    }

    #[test]
    fn misaligned_multi_assignment_ignored() {
        let source = "local a, b = io.open('x')";
        assert_eq!(shape_of(source, "a"), None);
        assert_eq!(shape_of(source, "b"), None);
    }

    #[test]
    fn method_called_with_dot_still_yields_return_shape() {
        let source = "local players = game.GetService(game, 'Players')";
        assert_eq!(shape_of(source, "players"), Some("Instance"));
    }
}
