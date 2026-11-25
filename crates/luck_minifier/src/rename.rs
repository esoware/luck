//! Renames every local, and with `rename_globals` each global the chunk
//! defines before reading it, to the shortest name its scopes allow.
//!
//! It reads a fresh analysis of the tree, as `oxc_mangler` does. Bindings
//! that are never live in the same scope share a slot, the slots spelled
//! most get the shortest names, and a slot never wears the name of a global
//! read where one of its bindings is visible. One walk then writes each
//! binding's name into the names that declare it and refer to it.

mod candidate_pool;
mod choose_names;
mod explicit_self;
mod rename_globals;
mod scope_claims;

use luck_allocator::Allocator;
use luck_ast::{Chunk, FunctionDecl, Name, TreeCounts, VisitMut};
use luck_data_structures::BitSet;
use luck_semantic::{NameId, ScopeId, Scoping, SymbolId, SymbolKind};

use self::candidate_pool::CandidatePool;
use self::rename_globals::renamable_globals;
use crate::binding::{METHOD_RECEIVER_NAME, NameIndexes, analyze};

struct Binding {
    scope: usize,
    /// Keeps its name: a function's implicit `self`, or an export.
    is_fixed: bool,
    /// How many names in the output spell it: its declaration's and its
    /// references'.
    occurrences: usize,
    /// The scope of the latest reference, so a run of references in one
    /// scope records it once.
    last_reference_scope: usize,
}

struct Scope {
    parent: Option<usize>,
    /// One past the last scope inside this one. Scope ids are in preorder,
    /// so this one and its descendants are `id..end`.
    end: usize,
}

/// A name the renamer can give out.
#[derive(Clone, Copy)]
struct Candidate<'ast> {
    name: &'ast str,
    /// A keyword, which no name can be.
    is_reserved: bool,
    /// The chunk's id for the name, if the chunk uses it.
    name_id: Option<NameId>,
}

/// No binding, or no scope.
const UNRESOLVED: usize = usize::MAX;

/// A global's place in [`Renamer::of_global`] when it keeps its name.
const NOT_RENAMED: u32 = u32::MAX;

/// What renaming learns of a tree from its scoping.
struct Renamer<'walk> {
    scoping: &'walk Scoping,
    scopes: Vec<Scope>,
    /// The renamed globals, then each symbol, in symbol order from
    /// `symbol_base`.
    renamed: Vec<Binding>,
    symbol_base: usize,
    /// Each global's renamed binding, by name, or [`NOT_RENAMED`].
    of_global: Vec<u32>,
    /// The implicit `self`s their methods read, which the methods take as a
    /// first parameter, since renaming them pays for it.
    receivers: BitSet,
    /// Each name's latest scope read as a global, so a run of reads in one
    /// scope records it once.
    last_global_read_scopes: Vec<usize>,
    /// Each renamed binding and a scope below its own it's read in.
    reference_scopes: Vec<(usize, usize)>,
    /// Each name read as a global or held by a binding that keeps it, and
    /// a scope it's read in.
    global_reads: Vec<(usize, usize)>,
}

/// Renames the locals of `chunk`, and with `should_rename_globals`, the
/// globals it assigns before anything reads them.
///
/// A method that reads `self` takes it as a parameter, so it's renamed too,
/// under an index from `name_indexes`.
///
/// The analysis it starts with reuses the tables of `reused`, an earlier
/// one's, when there is one.
pub(crate) fn rename<'ast>(
    allocator: &'ast Allocator,
    chunk: &mut Chunk<'ast>,
    counts: TreeCounts,
    should_rename_globals: bool,
    name_indexes: &mut NameIndexes,
    reused: Option<Scoping>,
) {
    let scoping = analyze(chunk, counts, reused);
    let mut renamer = Renamer::new(&scoping);
    // Opt-in, since renamed globals end up under new `_G` keys.
    if should_rename_globals {
        for name in renamable_globals(&chunk.block, &scoping) {
            // Its definitions are references, so no declaration spells it.
            let id = renamer.declare(0, false, 0);
            renamer.of_global[name.index()] =
                u32::try_from(id).expect("a chunk declares fewer bindings than `u32::MAX`");
        }
    }
    renamer.read_scoping();
    let mut pool = CandidatePool::new(allocator, &scoping);
    let new_names = renamer.choose_names(&mut pool);
    Applier {
        renamer: &renamer,
        new_names: &new_names,
        allocator,
        name_indexes,
        methods: Vec::new(),
    }
    .visit_block(&mut chunk.block);
}

impl<'walk> Renamer<'walk> {
    fn new(scoping: &'walk Scoping) -> Self {
        let mut scopes: Vec<Scope> = scoping
            .scopes()
            .iter()
            .enumerate()
            .map(|(id, scope)| Scope { parent: scope.parent.map(ScopeId::index), end: id + 1 })
            .collect();
        // A child's id is past its parent's, so the ends move up in one
        // pass from the last.
        for id in (0..scopes.len()).rev() {
            let (end, parent) = (scopes[id].end, scopes[id].parent);
            if let Some(parent) = parent {
                let enclosing = &mut scopes[parent];
                enclosing.end = enclosing.end.max(end);
            }
        }
        let name_count = scoping.names().len();
        Self {
            scoping,
            scopes,
            renamed: Vec::new(),
            symbol_base: 0,
            of_global: vec![NOT_RENAMED; name_count],
            receivers: BitSet::new(scoping.symbols().len()),
            last_global_read_scopes: vec![UNRESOLVED; name_count],
            reference_scopes: Vec::new(),
            global_reads: Vec::new(),
        }
    }

    /// Declares a binding of `scope` that `declarations` names spell before
    /// any reference does.
    fn declare(&mut self, scope: usize, is_fixed: bool, declarations: usize) -> usize {
        let id = self.renamed.len();
        self.renamed.push(Binding {
            scope,
            is_fixed,
            occurrences: declarations,
            last_reference_scope: scope,
        });
        id
    }

    /// Records a reference to the renamed binding `id` from `scope`.
    fn reference(&mut self, id: usize, scope: usize) {
        let binding = &mut self.renamed[id];
        binding.occurrences += 1;
        if binding.last_reference_scope != scope {
            binding.last_reference_scope = scope;
            self.reference_scopes.push((id, scope));
        }
    }

    fn read_globally(&mut self, name: usize, scope: usize) {
        let last_scope = &mut self.last_global_read_scopes[name];
        if *last_scope != scope {
            *last_scope = scope;
            self.global_reads.push((name, scope));
        }
    }

    /// Declares each symbol and records its references, and where each
    /// global and each name a binding keeps is read.
    ///
    /// A binding that keeps its name holds that name wherever it's read, as
    /// a global does, and counts as read in its own scope, even if nothing
    /// reads it, since a local renamed to that name would otherwise capture
    /// every use of it.
    fn read_scoping(&mut self) {
        let scoping = self.scoping;
        self.symbol_base = self.renamed.len();
        for symbol in scoping.symbols() {
            // The new `X,` parameter costs at most 2 bytes, and each `self`
            // saves 3 once renamed to a single character, so one read pays.
            let is_receiver =
                symbol.kind == SymbolKind::ImplicitSelf && self.is_read_at_runtime(symbol.id);
            if is_receiver {
                self.receivers.insert(symbol.id.index());
            }
            let is_fixed = (symbol.kind.is_implicit() && !is_receiver) || symbol.is_exported;
            let (name, scope) = (symbol.name.index(), symbol.scope.index());
            // A receiver's declaration is the parameter it becomes.
            let id = self.declare(scope, is_fixed, 1);
            if is_fixed {
                self.read_globally(name, scope);
            }
            for &reference in scoping.resolved_references(symbol.id) {
                let reference_scope = scoping.reference(reference).scope.index();
                self.reference(id, reference_scope);
                if is_fixed {
                    self.read_globally(name, reference_scope);
                }
            }
        }
        for reference in scoping.references() {
            if reference.resolved.is_some() {
                continue;
            }
            let (name, scope) = (reference.name.index(), reference.scope.index());
            let id = self.of_global[name];
            if id == NOT_RENAMED {
                self.read_globally(name, scope);
            } else {
                self.reference(id as usize, scope);
            }
        }
    }

    /// Whether a reference to `symbol` outside a type reads it.
    fn is_read_at_runtime(&self, symbol: SymbolId) -> bool {
        let mut references = self.scoping.resolved_references(symbol).iter();
        references.any(|&reference| !self.scoping.reference(reference).is_in_type)
    }

    /// The renamed binding `name` declares or refers to, if it's a local or
    /// a renamed global.
    fn renamed_of(&self, name: &Name<'_>) -> Option<usize> {
        if let Some(symbol) = self.scoping.symbol_of(name) {
            return Some(self.symbol_base + symbol.index());
        }
        let reference = self.scoping.reference(self.scoping.reference_of(name)?);
        if let Some(symbol) = reference.resolved {
            return Some(self.symbol_base + symbol.index());
        }
        let global = self.of_global[reference.name.index()];
        (global != NOT_RENAMED).then_some(global as usize)
    }
}

/// Writes each binding's new name into its names, and makes a method that
/// takes its `self` as a parameter declare it so.
struct Applier<'apply, 'walk, 'ast> {
    renamer: &'apply Renamer<'walk>,
    /// The name renaming gives each renamed binding, or none for one that
    /// keeps its own.
    new_names: &'apply [Option<&'ast str>],
    allocator: &'ast Allocator,
    name_indexes: &'apply mut NameIndexes,
    /// For each method the walk is in, innermost last, the implicit `self`
    /// its body reads, once a read outside a type shows which.
    methods: Vec<Option<SymbolId>>,
}

impl<'ast> VisitMut<'ast> for Applier<'_, '_, 'ast> {
    fn visit_name(&mut self, name: &mut Name<'ast>) {
        let scoping = self.renamer.scoping;
        // A method's body is the only place its `self` is read, and the
        // nearest method around a read binds the `self` it reads.
        if let Some(reference) = scoping.reference_of(name).map(|id| scoping.reference(id))
            && let Some(symbol) = reference.resolved
            && !reference.is_in_type
            && self.renamer.receivers.contains(symbol.index())
            && let Some(receiver) = self.methods.last_mut()
        {
            *receiver = Some(symbol);
        }
        if let Some(new_name) = self.renamer.renamed_of(name).and_then(|id| self.new_names[id]) {
            name.text = new_name.into();
        }
    }

    fn visit_function_decl(&mut self, function_decl: &mut FunctionDecl<'ast>) {
        let is_method = function_decl.name.method.is_some();
        if is_method {
            self.methods.push(None);
        }
        self.walk_function_decl(function_decl);
        if is_method && let Some(Some(receiver)) = self.methods.pop() {
            let renamed = self.renamer.symbol_base + receiver.index();
            let text = self.new_names[renamed].unwrap_or(METHOD_RECEIVER_NAME);
            let span = function_decl.name.method.map_or(function_decl.span, |method| method.span);
            let name = Name::new(text.into(), Some(self.name_indexes.next()), span);
            explicit_self::take_receiver(self.allocator, function_decl, name);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::{RENAMING, assert_renames, minify_with};

    fn apply(source: &str) -> String {
        minify_with(source, &RENAMING)
    }

    #[test]
    fn renames_locals_to_short_names() {
        let result = apply(
            "\
local longname = 1
return longname
",
        );
        assert!(!result.contains("longname"), "Local should be renamed: {result}");
    }

    #[test]
    fn preserves_globals() {
        let result = apply("print(42)\n");
        assert!(result.contains("print"), "Global should not be renamed: {result}");
    }

    #[test]
    fn renames_function_params() {
        let result = apply(
            "\
local function foo(longparam)
  return longparam
end
return foo
",
        );
        assert!(!result.contains("longparam"), "Param should be renamed: {result}");
    }

    #[test]
    fn preserves_field_names() {
        let result = apply(
            "\
local t = {}
t.field = 1
return t.field
",
        );
        assert!(result.contains("field"), "Field name should not be renamed: {result}");
    }

    #[test]
    fn renames_a_nested_scope_to_valid_lua() {
        apply(
            "\
local x = 1
do
  local y = 2
  print(y)
end
return x
",
        );
    }

    #[test]
    fn most_frequent_local_gets_the_first_name() {
        let result = apply(
            "\
local rare = 1
local frequent = 2
print(frequent)
print(frequent)
print(frequent)
print(rare)
return frequent, rare
",
        );
        assert_eq!(result, "local t=1 local e=2 print(e)print(e)print(e)print(t)return e,t");
    }

    #[test]
    fn sibling_scopes_reuse_names() {
        let result = apply(
            "\
do
  local longvar = 1
  print(longvar)
end
do
  local another = 2
  print(another)
end
",
        );
        assert!(
            !result.contains("longvar") && !result.contains("another"),
            "Both should be renamed: {result}"
        );
    }

    #[test]
    fn renames_nested_function_locals_to_valid_lua() {
        apply(
            "\
local outer = 1
local function f()
  local inner = 2
  print(inner, inner, inner, inner, inner)
end
return outer, f
",
        );
    }

    #[test]
    fn locals_can_shadow_unused_globals() {
        // `e` and `t` are globals at the top level but not inside `f`.
        let result = apply(
            "\
e(1)
t(2)
local function f()
  local longvar1 = 3
  local longvar2 = 4
  return longvar1 + longvar2
end
return f
",
        );
        assert_eq!(result, "e(1)t(2)local function n()local n=3 local e=4 return n+e end return n");
    }

    #[test]
    fn locals_avoid_globals_used_in_same_scope() {
        // `e` is a global called inside `f`, so locals must not shadow it.
        let result = apply(
            "\
local function f()
  local longvar = 1
  e(longvar)
  return longvar
end
return f
",
        );
        assert_eq!(result, "local function t()local t=1 e(t)return t end return t");
    }

    #[test]
    fn env_is_an_ordinary_name() {
        assert_renames(&[
            (
                "\
_ENV = 1
print(_ENV)
",
                "e=1 print(e)",
            ),
            (
                "\
function _ENV() end
print(_ENV)
",
                "function e()end print(e)",
            ),
            (
                "\
local _ENV = 1
print(_ENV)
",
                "local e=1 print(e)",
            ),
            ("for _ENV = 1, 2 do print(_ENV) end\n", "for e=1,2 do print(e)end"),
            ("for k, _ENV in pairs(t) do print(_ENV) end\n", "for t,e in pairs(t)do print(e)end"),
        ]);
    }

    #[test]
    fn renames_inside_a_function_embedded_in_a_type() {
        assert_renames(&[
            (
                "type T = typeof(function(x) return x end)\n",
                "type T=typeof(function(e)return e end)",
            ),
            (
                "type T<U = typeof(function(x) return x end)> = U\n",
                "type T<U=typeof(function(e)return e end)> =U",
            ),
            (
                "type function T() return typeof(function(x) return x end) end\n",
                "type function T()return typeof(function(e)return e end)end",
            ),
            (
                "type T = { f: typeof(function(x) return x end)? }\n",
                "type T={f:typeof(function(e)return e end)?}",
            ),
            (
                "type T = (typeof(function(x) return x end)) -> ()\n",
                "type T=(typeof(function(e)return e end))->()",
            ),
            (
                "type T = A<typeof(function(x) return x end)>\n",
                "type T=A<typeof(function(e)return e end)>",
            ),
            (
                "return 1 :: typeof(function(x) return x end)\n",
                "return 1::typeof(function(e)return e end)",
            ),
            (
                "return f<<typeof(function(x) return x end)>>\n",
                "return f<<typeof(function(e)return e end)>>",
            ),
            (
                "return f<<typeof(function(x) return x end)>>()\n",
                "return f<<typeof(function(e)return e end)>>()",
            ),
            ("f<<typeof(function(x) return x end)>>()\n", "f<<typeof(function(e)return e end)>>()"),
            (
                "return t:m<<typeof(function(x) return x end)>>()\n",
                "return t:m<<typeof(function(e)return e end)>>()",
            ),
        ]);
    }

    #[test]
    fn bindings_in_a_type_keep_their_place_in_walk_order() {
        // Binding ids follow visit order, so visiting a type out of turn hands
        // the renamer another binding's name.
        assert_renames(&[
            (
                "f<<typeof(function(a, b) return a, b end)>>(function(c) return c end)\n",
                "f<<typeof(function(e,t)return e,t end)>>(function(e)return e end)",
            ),
            (
                "t:m<<typeof(function(a, b) return a, b end)>>(function(c) return c end)\n",
                "t:m<<typeof(function(e,t)return e,t end)>>(function(e)return e end)",
            ),
            (
                "return (function(a, b) return a, b end) :: typeof(function(c) return c end)\n",
                "return function(e,t)return e,t end::typeof(function(e)return e end)",
            ),
            (
                "type T<U = typeof(function(a, b) return a, b end)> = typeof(function(c) return c end)\n",
                "type T<U=typeof(function(e,t)return e,t end)> =typeof(function(e)return e end)",
            ),
            (
                "\
type T = typeof(function(longparam) return longparam end)
local after = f()
return after
",
                "type T=typeof(function(e)return e end)local e=f()return e",
            ),
        ]);
    }

    #[test]
    fn bindings_in_an_assignment_keep_their_place_in_walk_order() {
        // Binding ids follow visit order, and the renamer visits an
        // assignment's targets before its values. Visiting them out of turn
        // hands a parameter another binding's name, which can capture `x`.
        assert_renames(&[
            (
                "\
local x = f()
t[function(a) return x, a end] = function(b) return b end
return x
",
                "local e=f()t[function(t)return e,t end]=function(e)return e end return e",
            ),
            (
                "\
local x = f()
g(function(a) return a end).k = function(b) return b + x end
return x
",
                "local e=f()g(function(e)return e end).k=function(t)return t+e end return e",
            ),
        ]);
    }

    #[test]
    fn typeof_read_keeps_a_local_live_in_its_scope() {
        assert_renames(&[(
            "\
local outer = f()
do
local inner = f()
type T = typeof(outer)
print(inner)
end
print(outer)
",
            "local e=f()do local t=f()type T=typeof(e)print(t)end print(e)",
        )]);
    }

    #[test]
    fn typeof_read_of_a_global_is_not_captured() {
        assert_renames(&[(
            "\
local function f()
local longname = g()
type T = typeof(e)
return longname
end
return f
",
            "local function t()local t=g()type T=typeof(e)return t end return t",
        )]);
    }

    #[test]
    fn unreferenced_param_never_captures_live_upvalue() {
        // An unreferenced param must not get the name of an upvalue read in
        // the body, which would capture it.
        let result = apply(
            "\
local cache = string.rep
function W.new(p)
\tprint(cache)
end
cache = f()
",
        );
        // The upvalue is the outer local, so the param must not wear its name.
        let (_, after_declaration) = result.split_once("function W.new(").expect("decl kept");
        let param = after_declaration.chars().next();
        let outer = result.chars().nth(6);
        assert_ne!(param, outer, "param must not shadow the captured upvalue: {result}");
    }

    #[test]
    fn nested_method_self_is_not_the_enclosing_self_parameter() {
        let result = apply(
            "\
function M.f(self)
  function t:g() return self end
  return self
end
",
        );
        // The method's `self` is its own receiver, not the renamed parameter.
        assert_eq!(result, "function M.f(e)function t.g(e)return e end return e end");
    }

    #[test]
    fn renamed_local_never_wears_an_exported_name() {
        let result = apply(
            "\
export local e = 1
local longname = f()
print(e, longname)
",
        );
        assert_eq!(result, "export local e=1 local t=f()print(e,t)");
    }

    #[test]
    fn local_named_self_is_an_ordinary_local() {
        assert_renames(&[(
            "\
local self = f()
print(self, self)
",
            "local e=f()print(e,e)",
        )]);
    }

    #[test]
    fn typeof_in_every_annotation_follows_the_rename() {
        for source in [
            "\
local outer = f()
local function g(a: typeof(outer)) return a end
return g, outer
",
            "\
local outer = f()
local function g(...: typeof(outer)) return ... end
return g, outer
",
            "\
local outer = f()
local function g(): typeof(outer) return outer end
return g
",
            "\
local outer = f()
for i: typeof(outer) = 1, 2 do print(i) end
return outer
",
            "\
local outer = f()
for k: typeof(outer) in g() do print(k) end
return outer
",
        ] {
            let result = apply(source);
            assert!(!result.contains("typeof(outer)"), "{source}: {result}");
        }
    }

    #[test]
    fn typeof_in_a_signature_reads_the_scope_around_the_function() {
        for source in [
            "\
local a = f()
local function g(a: number, b: typeof(a)) return b end
return g, a
",
            "\
local a = f()
local function g(a: number): typeof(a) return a end
return g, a
",
            "\
local g = f()
local function g(z: typeof(g)) return z end
return g
",
            "\
local k = f()
for k, v: typeof(k) in g() do print(k, v) end
return k
",
        ] {
            let result = apply(source);
            assert!(result.contains("typeof(e)"), "{source}: {result}");
        }
    }

    #[test]
    fn typeof_in_a_local_annotation_follows_the_rename() {
        let result = apply(
            "\
local outer = f()
local copy: typeof(outer) = outer
return copy, outer
",
        );
        assert!(
            !result.contains("typeof(outer)"),
            "`typeof` must name the local's new name, or the output fails type checking: {result}"
        );
    }
}
