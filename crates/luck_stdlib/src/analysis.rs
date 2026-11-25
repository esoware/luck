use luck_ast::{Call, Chunk, SuffixedExpression, TreeCounts};
use luck_diagnostics::Diagnostic;
use luck_semantic::{NameId, Reference, Scoping, Semantic, SemanticBuilder, SymbolId};
use luck_token::Variant;

use crate::resolve::{ChainCallees, ResolvedCallee, ShapeResolver, symbol_shapes};
use crate::stdlib_model::{StdlibEntry, StdlibLibrary};

/// A chunk's [`Semantic`], read against the stdlib of one variant.
#[derive(Debug)]
pub struct SemanticAnalysis<'ast> {
    semantic: Semantic<'ast>,
    stdlib: &'ast StdlibLibrary<'ast>,
    /// Each symbol's shape, by symbol index: a never-reassigned local
    /// initialized from a value of known shape, such as
    /// `local s = string.rep(...)`.
    symbol_shapes: Vec<Option<&'ast str>>,
    /// Each name's standing as a global the host defines, by name index.
    host_globals: Vec<HostGlobal>,
}

#[derive(Debug, Clone, Copy)]
struct HostGlobal {
    is_stdlib: bool,
    is_extra: bool,
}

/// `chunk`'s semantic, read against `stdlib`, and beside it the checker's
/// errors.
///
/// The semantic holds every part: the checker's errors, the node table,
/// which the shapes read, and the `require` sites. Its tables are sized from
/// `counts`, the parse's or the default. It's the same whether or not the
/// variant is Roblox, which only adds the Roblox runtime's stdlib.
pub fn analyze<'ast>(
    chunk: &'ast Chunk<'ast>,
    counts: TreeCounts,
    stdlib: &'ast StdlibLibrary<'ast>,
) -> (SemanticAnalysis<'ast>, Vec<Diagnostic>) {
    let built = SemanticBuilder::new()
        .with_counts(counts)
        .with_check(true)
        .with_nodes(true)
        .with_requires(true)
        .build(chunk);
    let semantic = built.semantic;
    let symbol_shapes = symbol_shapes(&semantic, stdlib);
    let host_globals = semantic
        .scoping()
        .names()
        .map(|(_, name)| HostGlobal {
            is_stdlib: stdlib.globals.contains_key(name),
            is_extra: false,
        })
        .collect();
    (SemanticAnalysis { semantic, stdlib, symbol_shapes, host_globals }, built.errors)
}

impl<'ast> SemanticAnalysis<'ast> {
    pub const fn semantic(&self) -> &Semantic<'ast> {
        &self.semantic
    }

    pub const fn scoping(&self) -> &Scoping {
        self.semantic.scoping()
    }

    pub const fn stdlib(&self) -> &'ast StdlibLibrary<'ast> {
        self.stdlib
    }

    pub const fn variant(&self) -> Variant {
        self.stdlib.variant
    }

    /// Marks `names` as globals the host defines beyond the stdlib, such as
    /// `vim`, from the `extra_globals` config. Only the chunk's own names
    /// are kept, since no other name can reach a query.
    pub fn add_extra_globals<'name, Names>(&mut self, names: Names)
    where
        Names: IntoIterator<Item = &'name str>,
    {
        for name in names {
            if let Some(id) = self.semantic.scoping().name_id(name) {
                self.host_globals[id.index()].is_extra = true;
            }
        }
    }

    /// Whether `name` is recognized as a global: a stdlib entry for this
    /// variant or an extra global.
    pub fn is_known_global(&self, name: &str) -> bool {
        match self.scoping().name_id(name) {
            Some(id) => self.is_known_global_name(id),
            None => self.stdlib.globals.contains_key(name),
        }
    }

    /// [`Self::is_known_global`] for one of the chunk's names.
    pub fn is_known_global_name(&self, name: NameId) -> bool {
        let global = self.host_globals[name.index()];
        global.is_stdlib || global.is_extra
    }

    /// Whether [`Self::add_extra_globals`] marked `name`.
    pub fn is_extra_global(&self, name: NameId) -> bool {
        self.host_globals[name.index()].is_extra
    }

    /// Whether the stdlib marks the global `name` read-only, so code
    /// shouldn't reassign it. Namespaces always are.
    pub fn is_read_only_global(&self, name: &str) -> bool {
        self.stdlib.globals.get(name).is_some_and(StdlibEntry::is_read_only)
    }

    /// `symbol`'s shape: a never-reassigned local initialized from a value
    /// of known shape, such as `local s = string.rep(...)`.
    pub fn symbol_shape(&self, symbol: SymbolId) -> Option<&'ast str> {
        self.symbol_shapes[symbol.index()]
    }

    /// Returns `None` for shadowed bases, unknown names, values of unknown
    /// shape, and calls whose `.` or `:` doesn't match the function's
    /// `is_method`.
    ///
    /// This reads the callee from its prefix, so for each call of a chain,
    /// [`Self::chain_callees`] costs less.
    pub fn resolve_callee<'call>(&'call self, call: Call<'call>) -> Option<ResolvedCallee<'call>> {
        self.shape_resolver().call_callee(call)
    }

    /// Resolves the calls of `chain`, asked for front to back, in one pass
    /// over it.
    pub fn chain_callees<'chain>(
        &'chain self,
        chain: &'chain SuffixedExpression<'chain>,
    ) -> ChainCallees<'chain> {
        ChainCallees::new(self.shape_resolver(), chain)
    }

    fn shape_resolver(&self) -> ShapeResolver<'_, 'ast> {
        ShapeResolver::new(self.scoping(), self.stdlib, &self.symbol_shapes)
    }

    /// The entry `segments` names, where `root` is the reference their first
    /// makes.
    ///
    /// With `local s = string.rep(...)`, `["s", "upper"]` resolves through
    /// the `string` shape. A root that reaches the host's global is a plain
    /// library lookup. Any other root resolves to nothing unless it's a local
    /// of known shape, which keeps shadowed names out of hover and semantic
    /// tokens.
    pub fn resolve_stdlib_path(
        &self,
        segments: &[&str],
        root: &Reference,
    ) -> Option<&'ast StdlibEntry<'ast>> {
        let members = segments.get(1..)?;
        match root.resolved {
            Some(symbol) => {
                let (first, rest) = members.split_first()?;
                let mut current = self.stdlib.shape_member(self.symbol_shape(symbol)?, first)?;
                for segment in rest {
                    current = self.stdlib.child(current, segment)?;
                }
                Some(current)
            }
            None if self.scoping().is_host_global(root) => self.stdlib.lookup(segments),
            None => None,
        }
    }
}
