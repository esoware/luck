//! There's no type inference.
//!
//! A value has a known shape only when it's a
//! stdlib value that declares one, a string literal, a call whose entry
//! declares a return shape, or a never-reassigned local initialized from
//! one of those.
//!
//! A chain resolves from its prefix forward, a suffix at a time, so what it
//! reads, or what any of its calls calls, costs one pass over it.

use luck_ast::{AstKind, Call, ChainHead, Expression, Name, Suffix, SuffixedExpression};
use luck_semantic::{Scoping, Semantic, SymbolId};

use crate::stdlib_model::{StdlibEntry, StdlibLibrary};

/// What a chain's head reads, as far as it's known.
#[derive(Debug, Clone, Copy, Default)]
struct HeadState<'lib> {
    shape: Option<&'lib str>,
    /// The stdlib entry the head names while it's a dotted path from the
    /// host's global.
    global: Option<&'lib StdlibEntry<'lib>>,
}

impl HeadState<'_> {
    /// No suffix after an unknown head is known either.
    const fn is_unknown(self) -> bool {
        self.shape.is_none() && self.global.is_none()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ShapeResolver<'input, 'lib> {
    tree: &'input Scoping,
    library: &'lib StdlibLibrary<'lib>,
    /// Each symbol's shape, by symbol index.
    symbol_shapes: &'input [Option<&'lib str>],
}

impl<'input, 'lib> ShapeResolver<'input, 'lib> {
    pub(crate) const fn new(
        tree: &'input Scoping,
        library: &'lib StdlibLibrary<'lib>,
        symbol_shapes: &'input [Option<&'lib str>],
    ) -> Self {
        Self { tree, library, symbol_shapes }
    }

    const fn symbol_shape(self, symbol: SymbolId) -> Option<&'lib str> {
        self.symbol_shapes[symbol.index()]
    }

    /// The shape of `expr`, when statically known. See the module docs for
    /// what "known" means.
    fn expr_shape(self, expr: &Expression<'_>) -> Option<&'lib str> {
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
    fn name_state(self, name: &Name<'_>) -> HeadState<'lib> {
        let Some(reference) = self.tree.reference_of(name).map(|id| self.tree.reference(id)) else {
            return HeadState::default();
        };
        match reference.resolved {
            Some(symbol) => HeadState { shape: self.symbol_shape(symbol), global: None },
            None if self.tree.is_host_global(reference) => {
                let global = self.library.globals.get(name.text.as_str());
                HeadState { shape: global.and_then(StdlibEntry::shape), global }
            }
            None => HeadState::default(),
        }
    }

    fn prefix_state(self, prefix: &Expression<'_>) -> HeadState<'lib> {
        if let Expression::Name(name) = prefix {
            self.name_state(name)
        } else {
            HeadState { shape: self.expr_shape(prefix), global: None }
        }
    }

    fn head_state(self, head: ChainHead<'_>) -> HeadState<'lib> {
        self.read(self.prefix_state(head.prefix), head.suffixes)
    }

    /// What a head that reads `state` reads once `suffixes` apply to it.
    fn read(self, mut state: HeadState<'lib>, suffixes: &[Suffix<'_>]) -> HeadState<'lib> {
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
    fn after_suffix(self, suffix: &Suffix<'_>, state: HeadState<'lib>) -> HeadState<'lib> {
        let callee = match suffix {
            Suffix::Field(name) => {
                let member =
                    state.global.and_then(|parent| self.library.child(parent, name.text.as_str()));
                return HeadState { shape: member.and_then(StdlibEntry::shape), global: member };
            }
            Suffix::Call(_) => state.global,
            Suffix::MethodCall(call) => {
                state.shape.and_then(|shape| self.shape_method(shape, call.name.text.as_str()))
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
    fn shape_method(self, shape: &str, method_name: &str) -> Option<&'lib StdlibEntry<'lib>> {
        let entry = self.library.shape_member(shape, method_name)?;
        matches!(entry, StdlibEntry::Function(function) if function.is_method).then_some(entry)
    }

    /// What a call with `method` after `head`, which reads `state`, calls.
    fn callee(
        self,
        head: ChainHead<'input>,
        state: HeadState<'lib>,
        method: Option<&'input Name<'input>>,
    ) -> Option<ResolvedCallee<'input>>
    where
        'lib: 'input,
    {
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
        let entry = self.shape_method(shape, method_name.as_str())?;
        let base = head.as_name().map_or(shape, |name| name.text.as_str());
        Some(ResolvedCallee {
            path: CalleePath::Method { base, method: method_name.as_str() },
            entry,
        })
    }

    /// What `call` calls, read from its callee's prefix.
    pub(crate) fn call_callee(self, call: Call<'input>) -> Option<ResolvedCallee<'input>>
    where
        'lib: 'input,
    {
        self.callee(call.callee, self.head_state(call.callee), call.method)
    }
}

/// The derived `string` shape, when the library has one (it does whenever a
/// `string` namespace exists).
fn string_shape<'lib>(library: &StdlibLibrary<'lib>) -> Option<&'lib str> {
    library.shapes.contains_key("string").then_some("string")
}

/// Each symbol's shape, by symbol index.
///
/// Only a never-reassigned local that a `local` statement with one value
/// per name declares, and whose value has a known shape, has one. Nodes are
/// in source order, so a local comes before any value that can read it, and
/// a shaped local seeds later ones.
pub(crate) fn symbol_shapes<'lib>(
    semantic: &Semantic<'_>,
    library: &'lib StdlibLibrary<'lib>,
) -> Vec<Option<&'lib str>> {
    let tree = semantic.scoping();
    let mut shapes = vec![None; tree.symbols().len()];
    for node in semantic.nodes().iter() {
        let AstKind::LocalAssignment(local) = node.kind() else {
            continue;
        };
        if local.exprs.len() != local.names.len() {
            continue;
        }
        for (attributed, initializer) in local.names.iter().zip(&local.exprs) {
            let Some(symbol) = tree.symbol_of(&attributed.name) else {
                continue;
            };
            if tree.symbol(symbol).is_modified {
                continue;
            }
            shapes[symbol.index()] =
                ShapeResolver::new(tree, library, &shapes).expr_shape(initializer);
        }
    }
    shapes
}

/// A call's stdlib entry, and the path that reaches it.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedCallee<'call> {
    path: CalleePath<'call>,
    entry: &'call StdlibEntry<'call>,
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

impl<'call> ResolvedCallee<'call> {
    /// Display path, such as `table.insert` or `game:GetService`.
    #[expect(
        clippy::missing_panics_doc,
        reason = "a callee resolves through its head only while the head is a dotted path"
    )]
    pub fn name(&self) -> String {
        match self.path {
            CalleePath::Method { base, method } => format!("{base}:{method}"),
            CalleePath::Dotted(head) => {
                head.dotted_path().expect("a dotted callee's head is a dotted path").to_string()
            }
        }
    }

    pub const fn entry(&self) -> &'call StdlibEntry<'call> {
        self.entry
    }
}

/// Resolves the calls of one chain front to back, carrying what each suffix
/// reads on to the next, so its calls resolve in one pass over it.
#[derive(Debug, Clone, Copy)]
pub struct ChainCallees<'chain> {
    resolver: ShapeResolver<'chain, 'chain>,
    chain: &'chain SuffixedExpression<'chain>,
    /// What the head of the first `read_count` suffixes reads.
    state: HeadState<'chain>,
    read_count: usize,
}

impl<'chain> ChainCallees<'chain> {
    pub(crate) fn new(
        resolver: ShapeResolver<'chain, 'chain>,
        chain: &'chain SuffixedExpression<'chain>,
    ) -> Self {
        let state = resolver.prefix_state(&chain.prefix);
        Self { resolver, chain, state, read_count: 0 }
    }

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
    use luck_semantic::SemanticBuilder;
    use luck_token::{LuaTarget, LuaVersion};

    use super::*;
    use crate::fixtures::parse;
    use crate::stdlib::layer;
    use crate::stdlib_definition::StdlibDefinition;

    const SHAPED_LIBRARY: &str = r#"
[globals.game]
kind = "property"
shape = "DataModel"

[globals.io]
kind = "namespace"

[globals.io.members.open]
kind = "function"
params = [{ kind = "string" }, { kind = "string", is_required = false }]
returns = "file"

[globals.string]
kind = "namespace"

[globals.string.members.upper]
kind = "function"
is_must_use = true
params = [{ kind = "string" }]
returns = "string"

[shapes.DataModel.members.GetService]
kind = "function"
is_must_use = true
is_method = true
params = [{ kind = "string" }]
returns = "Instance"

[shapes.Instance.members.FindFirstChild]
kind = "function"
is_method = true
params = [{ kind = "string" }, { kind = "bool", is_required = false }]

[shapes.file.members.read]
kind = "function"
is_method = true
params = [{ kind = "vararg" }]
"#;

    fn shape_of(source: &str, name: &str) -> Option<String> {
        let allocator = Allocator::new();
        let definition = StdlibDefinition::parse(SHAPED_LIBRARY).expect("the library parses");
        let library = layer(&allocator, &StdlibLibrary::empty(LuaTarget::Lua54), &definition)
            .expect("the library layers");
        let chunk = parse(&allocator, source, LuaVersion::Lua54);
        let semantic = SemanticBuilder::new().with_nodes(true).build(chunk).semantic;
        let shapes = symbol_shapes(&semantic, &library);
        let tree = semantic.scoping();
        let symbol = tree.symbols().iter().find(|symbol| tree.name(symbol.name) == name)?;
        shapes[symbol.id.index()].map(str::to_owned)
    }

    #[test]
    fn local_from_shaped_call_gets_shape() {
        assert_eq!(shape_of("local f = io.open('x')", "f").as_deref(), Some("file"));
    }

    #[test]
    fn local_from_shaped_global_gets_shape() {
        assert_eq!(shape_of("local g = game", "g").as_deref(), Some("DataModel"));
    }

    #[test]
    fn method_chain_propagates_shapes() {
        let source = "\
local players = game:GetService('Players')
local again = players:FindFirstChild('x')";
        assert_eq!(shape_of(source, "players").as_deref(), Some("Instance"));
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
        assert_eq!(shape_of("local s = 'abc'", "s").as_deref(), Some("string"));
    }

    #[test]
    fn parenthesized_literal_method_chains_string_shape() {
        // The string shape's upper returns a string, so the call result
        // seeds the local's shape.
        assert_eq!(shape_of("local u = ('x'):upper()", "u").as_deref(), Some("string"));
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
        assert_eq!(shape_of(source, "players").as_deref(), Some("Instance"));
    }
}
