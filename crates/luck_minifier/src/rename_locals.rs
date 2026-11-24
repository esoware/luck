//! Renames every local to the shortest name its scopes allow.
//!
//! It reads a fresh analysis of the tree, as `oxc_mangler` does. Bindings
//! that are never live in the same scope share a slot, the most-read slots
//! get the shortest names, and a slot never wears the name of a global read
//! where it's live. One walk then writes each binding's name into the names
//! that declare it and refer to it.

mod candidate_pool;
mod choose_names;
mod explicit_self;
mod scope_claims;

use luck_allocator::Allocator;
use luck_ast::{Chunk, FunctionDecl, Name, TreeCounts, VisitMut};
use luck_data_structures::BitSet;
use luck_semantic::{ScopeId, Scoping, SymbolId, SymbolKind};
use luck_token::{index_to_offset, offset_to_index};

use self::candidate_pool::CandidatePool;
use self::choose_names::Choice;
use crate::binding::{METHOD_RECEIVER_NAME, NameIndexes};
use crate::compress::analyze;
use crate::rename_globals::renamable_globals;

struct Binding {
    scope: usize,
    /// Keeps its name: a function's implicit `self` or 5.1 `arg`, a 5.2+
    /// `_ENV`, or a Luau export.
    is_fixed: bool,
    references: usize,
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

/// No binding.
const UNRESOLVED: usize = usize::MAX;

/// A global's place in [`Renamer::of_global`] when it keeps its name.
const NOT_RENAMED: u32 = u32::MAX;

/// What renaming learns of a tree from its scoping.
struct Renamer<'walk, 'ast> {
    scoping: &'walk Scoping,
    scopes: Vec<Scope>,
    /// The renamed globals, then each symbol, in symbol order from
    /// `symbol_base`.
    renamed: Vec<Binding>,
    symbol_base: usize,
    /// Each global's renamed binding, by name, or [`NOT_RENAMED`].
    of_global: Vec<u32>,
    /// The implicit `self`s read often enough that their methods take them
    /// as a first parameter, which renaming them pays for.
    receivers: BitSet,
    /// Each name's latest scope read as a global, so a run of reads in one
    /// scope records it once.
    last_global_read_scopes: Vec<usize>,
    /// Each renamed binding and a scope below its own it's read in.
    reference_scopes: Vec<(usize, usize)>,
    /// Each name read as a global or held by a binding that keeps it, and
    /// a scope it's read in.
    global_reads: Vec<(usize, usize)>,
    choice: Choice<'ast>,
    pool: CandidatePool<'ast>,
    new_names: Vec<&'ast str>,
}

/// Renames the locals of `chunk`, and with `should_rename_globals`, the
/// globals it assigns before anything reads them.
///
/// A method that reads `self` often enough takes it as a parameter, so it's
/// renamed too, under an index from `names`.
pub(crate) fn rename<'ast>(
    allocator: &'ast Allocator,
    chunk: &mut Chunk<'ast>,
    counts: TreeCounts,
    should_rename_globals: bool,
    names: &mut NameIndexes,
) {
    let scoping = analyze(chunk, counts);
    let mut renamer = Renamer::new(allocator, &scoping);
    // Opt-in, since renamed globals end up under new `_G` keys.
    if should_rename_globals {
        for name in renamable_globals(&chunk.block, chunk.version, &scoping) {
            let id = renamer.declare(0, false);
            if let Some(slot) = renamer.of_global.get_mut(name.index()) {
                *slot = index_to_offset(id);
            }
        }
    }
    renamer.read_scoping();
    renamer.choose_names();
    Applier { renamer: &renamer, allocator, names, methods: Vec::new() }
        .visit_block(&mut chunk.block);
}

impl<'walk, 'ast> Renamer<'walk, 'ast> {
    fn new(allocator: &'ast Allocator, scoping: &'walk Scoping) -> Self {
        let mut scopes: Vec<Scope> = scoping
            .scopes()
            .iter()
            .enumerate()
            .map(|(id, scope)| Scope {
                parent: scope.parent.map(ScopeId::index),
                end: id.saturating_add(1),
            })
            .collect();
        // A child's id is past its parent's, so the ends move up in one
        // pass from the last.
        for id in (0..scopes.len()).rev() {
            let (end, parent) = scopes.get(id).map_or((0, None), |scope| (scope.end, scope.parent));
            if let Some(enclosing) = parent.and_then(|parent_id| scopes.get_mut(parent_id)) {
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
            choice: Choice::default(),
            pool: CandidatePool::new(allocator, scoping.version()),
            new_names: Vec::new(),
        }
    }

    fn declare(&mut self, scope: usize, is_fixed: bool) -> usize {
        let id = self.renamed.len();
        self.renamed.push(Binding { scope, is_fixed, references: 0, last_reference_scope: scope });
        id
    }

    /// Records a reference to the renamed binding `id` from `scope`.
    fn reference(&mut self, id: usize, scope: usize) {
        if let Some(binding) = self.renamed.get_mut(id) {
            binding.references = binding.references.saturating_add(1);
            if binding.last_reference_scope != scope {
                binding.last_reference_scope = scope;
                self.reference_scopes.push((id, scope));
            }
        }
    }

    fn read_globally(&mut self, name: usize, scope: usize) {
        if let Some(last_scope) = self.last_global_read_scopes.get_mut(name)
            && *last_scope != scope
        {
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
    /// every use of it. So does a Lua 5.5 `global` declaration, after which
    /// the name reads the global.
    fn read_scoping(&mut self) {
        let scoping = self.scoping;
        self.symbol_base = self.renamed.len();
        for symbol in scoping.symbols() {
            // The new `X,` parameter costs 2 bytes, and each `self` saves 3
            // once renamed to a single character.
            let is_receiver =
                symbol.kind == SymbolKind::ImplicitSelf && self.runtime_references(symbol.id) >= 2;
            if is_receiver {
                self.receivers.insert(symbol.id.index());
            }
            let is_fixed = (symbol.kind.is_implicit() && !is_receiver)
                || symbol.is_exported
                || scoping.is_env_binding(symbol.id);
            let (name, scope) = (symbol.name.index(), symbol.scope.index());
            let id = self.declare(scope, is_fixed);
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
            match self.of_global.get(name).copied().filter(|&id| id != NOT_RENAMED) {
                Some(id) => self.reference(offset_to_index(id), scope),
                None => self.read_globally(name, scope),
            }
        }
        for &(name, scope) in scoping.declared_globals() {
            self.read_globally(name.index(), scope.index());
        }
    }

    /// How many references to `symbol` aren't in a type.
    fn runtime_references(&self, symbol: SymbolId) -> usize {
        let references = self.scoping.resolved_references(symbol).iter();
        references.filter(|&&reference| !self.scoping.reference(reference).is_in_type).count()
    }

    /// The renamed binding `name` declares or refers to, if it's a local or
    /// a renamed global.
    fn renamed_of(&self, name: &Name<'_>) -> Option<usize> {
        if let Some(symbol) = self.scoping.symbol_of(name) {
            return Some(self.symbol_base.saturating_add(symbol.index()));
        }
        let reference = self.scoping.reference(self.scoping.reference_of(name)?);
        reference.resolved.map_or_else(
            || {
                let global = self.of_global.get(reference.name.index()).copied();
                global.filter(|&id| id != NOT_RENAMED).map(offset_to_index)
            },
            |symbol| Some(self.symbol_base.saturating_add(symbol.index())),
        )
    }

    /// The name renaming gives `id`, which is empty for one that keeps its
    /// own.
    fn new_name(&self, id: usize) -> &'ast str {
        self.new_names.get(id).copied().unwrap_or_default()
    }
}

/// Writes each binding's new name into its names, and makes a method that
/// takes its `self` as a parameter declare it so.
struct Applier<'apply, 'walk, 'ast> {
    renamer: &'apply Renamer<'walk, 'ast>,
    allocator: &'ast Allocator,
    names: &'apply mut NameIndexes,
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
        if let Some(new_name) = self.renamer.renamed_of(name).map(|id| self.renamer.new_name(id))
            && !new_name.is_empty()
        {
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
            let renamed = self.renamer.symbol_base.saturating_add(receiver.index());
            let new_name = self.renamer.new_name(renamed);
            let text = if new_name.is_empty() { METHOD_RECEIVER_NAME } else { new_name };
            let span = function_decl.name.method.map_or(function_decl.span, |method| method.span);
            let name = Name::new(text.into(), Some(self.names.next()), span);
            explicit_self::take_receiver(self.allocator, function_decl, name);
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::fixtures::{RENAMING, assert_lua55_renames, assert_luau_renames, minify_with};

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    fn apply_version(source: &str, version: LuaVersion) -> String {
        minify_with(source, version, &RENAMING)
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
    fn most_frequent_local_gets_shortest_name() {
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
        assert!(!result.contains("frequent"), "'frequent' should be renamed: {result}");
        assert!(!result.contains("rare"), "'rare' should be renamed: {result}");
    }

    #[test]
    fn preserves_self_parameter() {
        let result = apply(
            "\
local t = {}
function t:method()
  return self
end
return t
",
        );
        assert!(result.contains("self"), "'self' should not be renamed: {result}");
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
        // 'l' and 'u' are globals at top level but not inside f.
        let result = apply(
            "\
l(1)
u(2)
local function f()
  local longvar1 = 3
  local longvar2 = 4
  return longvar1 + longvar2
end
return f
",
        );
        assert!(!result.contains("longvar"), "locals should be renamed: {result}");
    }

    #[test]
    fn locals_avoid_globals_used_in_same_scope() {
        // 'l' is a global called inside f, so locals must not shadow it.
        let result = apply(
            "\
local function f()
  local longvar = 1
  l(longvar)
  return longvar
end
return f
",
        );
        assert!(result.contains("l("), "global 'l' must be preserved: {result}");
        assert!(!result.contains("longvar"), "longvar should be renamed: {result}");
    }

    #[test]
    fn renames_inside_global_function_body() {
        // The analyzer and the renamer must both walk `global function` bodies,
        // or the binding ids fall out of step.
        let result = apply_version(
            "\
global function f(longparam)
  return longparam
end
",
            LuaVersion::Lua55,
        );
        assert!(
            !result.contains("longparam"),
            "param inside global function should be renamed: {result}"
        );
        assert!(result.contains("function f("), "global function name must be preserved: {result}");
    }

    fn apply_luau(source: &str) -> String {
        apply_version(source, LuaVersion::Luau)
    }

    const ENV_VERSIONS: [LuaVersion; 4] =
        [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55];

    #[test]
    fn env_is_never_renamed_on_versions_with_an_env_upvalue() {
        for version in ENV_VERSIONS {
            for (source, expected) in [
                (
                    "\
_ENV = {}
print(1)
",
                    "_ENV={}print(1)",
                ),
                (
                    "\
_ENV = {}
_ENV = {}
",
                    "_ENV={}_ENV={}",
                ),
                (
                    "\
_ENV, x = {}, 1
print(x)
",
                    "_ENV,l={},1 print(l)",
                ),
                ("function _ENV() end\n", "function _ENV()end"),
                ("function _ENV.f() end\n", "function _ENV.f()end"),
                (
                    "\
local _ENV = {}
print(1)
",
                    "local _ENV={}print(1)",
                ),
                (
                    "\
local _ENV = {}
_ENV = {}
",
                    "local _ENV={}_ENV={}",
                ),
                (
                    "\
do local _ENV = {}
_ENV = {} end
_ENV = {}
",
                    "do local _ENV={}_ENV={}end _ENV={}",
                ),
                ("local function _ENV() end\n", "local function _ENV()end"),
                (
                    "\
local function f(_ENV) return x end
return f
",
                    "local function l(_ENV)return x end return l",
                ),
                ("for _ENV = 1, 2 do print(x) end\n", "for _ENV=1,2 do print(x)end"),
                (
                    "for k, _ENV in pairs(t) do print(k) end\n",
                    "for l,_ENV in pairs(t)do print(l)end",
                ),
            ] {
                let result = apply_version(source, version);
                assert_eq!(result, expected, "{version:?} source: {source}");
            }
        }
    }

    #[test]
    fn env_is_an_ordinary_name_where_there_is_no_env_upvalue() {
        for version in [LuaVersion::Lua51, LuaVersion::Luau] {
            for (source, expected) in [
                (
                    "\
_ENV = 1
print(_ENV)
",
                    "l=1 print(l)",
                ),
                (
                    "\
function _ENV() end
print(_ENV)
",
                    "function l()end print(l)",
                ),
                (
                    "\
local _ENV = 1
print(_ENV)
",
                    "local l=1 print(l)",
                ),
                ("for _ENV = 1, 2 do print(_ENV) end\n", "for l=1,2 do print(l)end"),
                (
                    "for k, _ENV in pairs(t) do print(_ENV) end\n",
                    "for u,l in pairs(t)do print(l)end",
                ),
            ] {
                let result = apply_version(source, version);
                assert_eq!(result, expected, "{version:?} source: {source}");
            }
        }
    }

    #[test]
    fn global_declaration_shadows_an_earlier_local() {
        // After the declaration `g` is the global, and until then it's the
        // local, so only the uses before it may take the local's new name.
        assert_lua55_renames(&[
            (
                "\
local g = f()
print(g)
global g, print
g = 2
print(g)
",
                "local l=f()print(l)global g,print g=2 print(g)",
            ),
            (
                "\
local g = f()
do global g
g = 2 end
print(g)
",
                "local l=f()do global g g=2 end print(l)",
            ),
            (
                "\
local g = f()
global function g() return g end
",
                "local l=f()global function g()return g end",
            ),
        ]);
    }

    #[test]
    fn global_declaration_takes_effect_after_its_initializers() {
        assert_lua55_renames(&[(
            "\
local g = f()
global g = g
g = 2
",
            "local l=f()global g=l g=2",
        )]);
    }

    #[test]
    fn local_declared_after_a_global_declaration_is_still_renamed() {
        assert_lua55_renames(&[(
            "\
global g, print
do
local g = f()
print(g)
end
g = 1
",
            "global g,print do local l=f()print(l)end g=1",
        )]);
    }

    #[test]
    fn renamed_local_never_wears_a_globally_declared_name() {
        // Lookup finds the newest declaration, so a local wearing `l` here
        // would read the global `l` as soon as the declaration is in scope,
        // whether or not anything else reads that global.
        assert_lua55_renames(&[
            (
                "\
local longname = f()
global l, print
print(longname)
",
                "local u=f()global l,print print(u)",
            ),
            (
                "\
local longname = f()
do global l, print
print(longname) end
",
                "local u=f()do global l,print print(u)end",
            ),
            (
                "\
global l, print, f
do
local longname = f()
print(longname, l)
end
",
                "global l,print,f do local u=f()print(u,l)end",
            ),
            (
                "\
local longname = f()
global function l() end
return longname
",
                "local u=f()global function l()end return u",
            ),
        ]);
    }

    #[test]
    fn renames_inside_a_function_embedded_in_a_type() {
        assert_luau_renames(&[
            (
                "type T = typeof(function(x) return x end)\n",
                "type T=typeof(function(l)return l end)",
            ),
            (
                "type T<U = typeof(function(x) return x end)> = U\n",
                "type T<U=typeof(function(l)return l end)> =U",
            ),
            (
                "type function T() return typeof(function(x) return x end) end\n",
                "type function T()return typeof(function(l)return l end)end",
            ),
            (
                "type T = { f: typeof(function(x) return x end)? }\n",
                "type T={f:typeof(function(l)return l end)?}",
            ),
            (
                "type T = (typeof(function(x) return x end)) -> ()\n",
                "type T=(typeof(function(l)return l end))->()",
            ),
            (
                "type T = A<typeof(function(x) return x end)>\n",
                "type T=A<typeof(function(l)return l end)>",
            ),
            (
                "return 1 :: typeof(function(x) return x end)\n",
                "return 1::typeof(function(l)return l end)",
            ),
            (
                "return f<<typeof(function(x) return x end)>>\n",
                "return f<<typeof(function(l)return l end)>>",
            ),
            (
                "return f<<typeof(function(x) return x end)>>()\n",
                "return f<<typeof(function(l)return l end)>>()",
            ),
            ("f<<typeof(function(x) return x end)>>()\n", "f<<typeof(function(l)return l end)>>()"),
            (
                "return t:m<<typeof(function(x) return x end)>>()\n",
                "return t:m<<typeof(function(l)return l end)>>()",
            ),
        ]);
    }

    #[test]
    fn bindings_in_a_type_keep_their_place_in_walk_order() {
        // Binding ids follow visit order, so visiting a type out of turn hands
        // the renamer another binding's name.
        assert_luau_renames(&[
            (
                "f<<typeof(function(a, b) return a, b end)>>(function(c) return c end)\n",
                "f<<typeof(function(l,u)return l,u end)>>(function(l)return l end)",
            ),
            (
                "t:m<<typeof(function(a, b) return a, b end)>>(function(c) return c end)\n",
                "t:m<<typeof(function(l,u)return l,u end)>>(function(l)return l end)",
            ),
            (
                "return (function(a, b) return a, b end) :: typeof(function(c) return c end)\n",
                "return function(l,u)return l,u end::typeof(function(l)return l end)",
            ),
            (
                "type T<U = typeof(function(a, b) return a, b end)> = typeof(function(c) return c end)\n",
                "type T<U=typeof(function(l,u)return l,u end)> =typeof(function(l)return l end)",
            ),
            (
                "\
type T = typeof(function(longparam) return longparam end)
local after = f()
return after
",
                "type T=typeof(function(l)return l end)local l=f()return l",
            ),
        ]);
    }

    #[test]
    fn bindings_in_an_assignment_keep_their_place_in_walk_order() {
        // Binding ids follow visit order, and the renamer visits an
        // assignment's targets before its values. Visiting them out of turn
        // hands a parameter another binding's name, which can capture `x`.
        assert_luau_renames(&[
            (
                "\
local x = f()
t[function(a) return x, a end] = function(b) return b end
return x
",
                "local l=f()t[function(u)return l,u end]=function(l)return l end return l",
            ),
            (
                "\
local x = f()
g(function(a) return a end).k = function(b) return b + x end
return x
",
                "local l=f()g(function(l)return l end).k=function(u)return u+l end return l",
            ),
        ]);
    }

    #[test]
    fn typeof_read_keeps_a_local_live_in_its_scope() {
        assert_luau_renames(&[(
            "\
local outer = f()
do
local inner = f()
type T = typeof(outer)
print(inner)
end
print(outer)
",
            "local l=f()do local u=f()type T=typeof(l)print(u)end print(l)",
        )]);
    }

    #[test]
    fn typeof_read_of_a_global_is_not_captured() {
        assert_luau_renames(&[(
            "\
local function f()
local longname = g()
type T = typeof(l)
return longname
end
return f
",
            "local function u()local u=g()type T=typeof(l)return u end return u",
        )]);
    }

    #[test]
    fn named_vararg_binds_its_name_in_the_body() {
        assert_lua55_renames(&[
            (
                "\
local args = f()
local function g(...args) return args end
return args, g
",
                "local l=f()local function u(...l)return l end return l,u",
            ),
            (
                "\
local function g(a, ...rest) return rest, a end
return g
",
                "local function l(l,...u)return u,l end return l",
            ),
        ]);
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
        assert!(
            result.contains(":g()return self"),
            "`self` in a `:` method is its own receiver, not the renamed parameter: {result}"
        );
    }

    #[test]
    fn lua51_vararg_function_keeps_its_implicit_arg() {
        let result = apply_version(
            "\
local arg = g()
local function f(...) return arg end
return f, arg
",
            LuaVersion::Lua51,
        );
        assert!(
            result.contains("return arg"),
            "`arg` in a 5.1 vararg function is its implicit table, not the outer local: {result}"
        );
    }

    #[test]
    fn renamed_local_never_wears_an_exported_name() {
        let result = apply_luau(
            "\
export local l = 1
local longname = f()
print(l, longname)
",
        );
        assert!(
            !result.contains("local l=f()"),
            "a renamed local must not shadow the exported `l`: {result}"
        );
    }

    #[test]
    fn lua51_implicit_arg_wins_over_a_parameter_named_arg() {
        let result = apply_version(
            "\
local function f(arg, ...) return arg end
return f
",
            LuaVersion::Lua51,
        );
        assert!(result.contains("return arg"), "{result}");
    }

    #[test]
    fn local_named_self_is_an_ordinary_local() {
        assert_luau_renames(&[(
            "\
local self = f()
print(self, self)
",
            "local l=f()print(l,l)",
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
            let result = apply_luau(source);
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
            let result = apply_luau(source);
            assert!(result.contains("typeof(l)"), "{source}: {result}");
        }
    }

    #[test]
    fn typeof_in_a_local_annotation_follows_the_rename() {
        let result = apply_luau(
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
