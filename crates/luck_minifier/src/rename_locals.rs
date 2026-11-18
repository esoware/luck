//! Renames every local to the shortest name its scopes allow.
//!
//! One walk records where each binding is declared and read, reading each
//! name's binding from the name itself. Bindings that are never live in the
//! same scope share a slot, the most-read slots get the shortest names, and
//! a slot never wears the name of a global read where it's live. A second
//! walk writes each binding's name into its declaration and its references.

mod candidate_pool;
mod choose_names;
mod explicit_self;
mod scope_claims;

use core::ptr;

use luck_allocator::Allocator;
use luck_ast::{
    Block, DoBlock, Expression, FunctionBody, FunctionDecl, GenericFor, GlobalDeclaration,
    GlobalFunction, IfStatement, LocalAssignment, LocalFunction, Name, NumericFor, RepeatLoop, Var,
    VisitMut, WhileLoop,
};
use luck_token::LuaVersion;

use self::candidate_pool::CandidatePool;
use self::choose_names::Choice;
use crate::binding::{Bindings, NONE, global_of, local_of};
use crate::name_table::NameTable;
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

/// The renamer, which a minify runs once.
struct Renamer<'walk, 'ast> {
    allocator: &'ast Allocator,
    is_applying: bool,
    bindings: &'walk mut Bindings,
    names: &'walk NameTable<'ast>,
    /// Each local binding's renamed binding, and each global name's, by
    /// binding id and by name id.
    of_local: Vec<u32>,
    of_global: Vec<u32>,
    scopes: Vec<Scope>,
    current_scope: usize,
    renamed: Vec<Binding>,
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

/// Renames the locals of `block`, whose bindings and names `bindings` and
/// `names` hold, and with `should_rename_globals`, the globals it assigns
/// before anything reads them.
///
/// A method that reads `self` often enough first takes it as a parameter,
/// so it's renamed too.
pub(crate) fn rename<'ast>(
    allocator: &'ast Allocator,
    version: LuaVersion,
    block: &mut Block<'ast>,
    bindings: &mut Bindings,
    names: &NameTable<'ast>,
    should_rename_globals: bool,
) {
    let binding_count = bindings.entries.len();
    let mut renamer = Renamer {
        allocator,
        is_applying: false,
        bindings,
        names,
        of_local: vec![NONE; binding_count],
        of_global: vec![NONE; names.len()],
        scopes: vec![Scope { parent: None, end: 1 }],
        current_scope: 0,
        renamed: Vec::new(),
        last_global_read_scopes: vec![UNRESOLVED; names.len()],
        reference_scopes: Vec::new(),
        global_reads: Vec::new(),
        choice: Choice::default(),
        pool: CandidatePool::new(allocator, version),
        new_names: Vec::new(),
    };
    // Opt-in, since renamed globals end up under new `_G` keys.
    if should_rename_globals {
        for name in renamable_globals(block, version, renamer.bindings, names) {
            let id = renamer.declare_renamed(false);
            if let Some(slot) = renamer.of_global.get_mut(name) {
                *slot = u32::try_from(id).unwrap_or(NONE);
            }
        }
    }
    renamer.visit_block(block);
    renamer.choose_names();
    renamer.is_applying = true;
    renamer.visit_block(block);
}

impl<'ast> Renamer<'_, 'ast> {
    fn enter_scope(&mut self) {
        if self.is_applying {
            return;
        }
        let id = self.scopes.len();
        self.scopes.push(Scope { parent: Some(self.current_scope), end: id.saturating_add(1) });
        self.current_scope = id;
    }

    fn exit_scope(&mut self) {
        if self.is_applying {
            return;
        }
        let end = self.scopes.len();
        let Some(scope) = self.scopes.get_mut(self.current_scope) else {
            return;
        };
        scope.end = end;
        self.current_scope = scope.parent.unwrap_or(0);
    }

    fn declare_renamed(&mut self, is_fixed: bool) -> usize {
        let id = self.renamed.len();
        let scope = self.current_scope;
        self.renamed.push(Binding { scope, is_fixed, references: 0, last_reference_scope: scope });
        id
    }

    /// The renamed binding a name declares or refers to, if it's a local or
    /// a renamed global.
    fn renamed_of(&self, name: &Name<'_>) -> usize {
        let slot = match (local_of(name), global_of(name)) {
            (Some(id), _) => self.of_local.get(self.bindings.resolve(id)),
            (None, Some(global)) => self.of_global.get(global),
            (None, None) => None,
        };
        slot.and_then(|&renamed| usize::try_from(renamed).ok()).unwrap_or(UNRESOLVED)
    }

    /// Declares the binding `name` holds. One that keeps its name counts as
    /// read in its own scope, even if nothing reads it, since a local
    /// renamed to that name would otherwise capture every use of it.
    fn declare(&mut self, name: &mut Name<'ast>) {
        if self.is_applying {
            self.apply(name);
            return;
        }
        let Some(id) = local_of(name).map(|id| self.bindings.resolve(id)) else {
            if let Some(global) = global_of(name) {
                self.read_globally(global);
            }
            return;
        };
        let Some(binding) = self.bindings.entries.get(id) else {
            return;
        };
        let (is_fixed, name_id) = (binding.is_fixed, binding.name);
        let renamed = self.declare_renamed(is_fixed);
        if self.of_local.len() <= id {
            self.of_local.resize(id.saturating_add(1), NONE);
        }
        if let Some(slot) = self.of_local.get_mut(id) {
            *slot = u32::try_from(renamed).unwrap_or(NONE);
        }
        if is_fixed {
            self.read_globally(usize::try_from(name_id).unwrap_or(usize::MAX));
        }
    }

    fn reference(&mut self, name: &mut Name<'ast>) {
        if self.is_applying {
            self.apply(name);
            return;
        }
        let renamed = self.renamed_of(name);
        let current_scope = self.current_scope;
        if let Some(binding) = self.renamed.get_mut(renamed) {
            binding.references = binding.references.saturating_add(1);
            if binding.last_reference_scope != current_scope {
                binding.last_reference_scope = current_scope;
                self.reference_scopes.push((renamed, current_scope));
            }
            if !binding.is_fixed {
                return;
            }
        }
        // A binding that keeps its name holds that name wherever it's read,
        // as a global does.
        let held_name = match local_of(name) {
            Some(id) => {
                self.bindings.get(id).and_then(|binding| usize::try_from(binding.name).ok())
            }
            None => global_of(name),
        };
        if let Some(read_name) = held_name {
            self.read_globally(read_name);
        }
    }

    fn apply(&self, name: &mut Name<'ast>) {
        let renamed = self.renamed_of(name);
        if let Some(&new_name) = self.new_names.get(renamed)
            && !new_name.is_empty()
            && !ptr::eq(name.text.as_str(), new_name)
            && name.text != new_name
        {
            name.text = new_name.into();
        }
    }

    fn read_globally(&mut self, name: usize) {
        let current_scope = self.current_scope;
        if let Some(scope) = self.last_global_read_scopes.get_mut(name)
            && *scope != current_scope
        {
            *scope = current_scope;
            self.global_reads.push((name, current_scope));
        }
    }

    /// Luau: a function's signature annotations, before anything the
    /// function binds. Parameters and a `local function`'s own name bind
    /// after the whole signature parses, so a `typeof` here reads the
    /// enclosing scope.
    fn visit_signature(&mut self, body: &mut FunctionBody<'ast>) {
        if let Some(generics) = &mut body.generics {
            self.visit_generic_type_list(generics);
        }
        for param in &mut body.params {
            if let Some(ty) = &mut param.type_annotation {
                self.visit_type(ty);
            }
        }
        if let Some(vararg) = &mut body.vararg
            && let Some(ty) = &mut vararg.type_annotation
        {
            self.visit_type(ty);
        }
        if let Some(ty) = &mut body.return_type {
            self.visit_type(ty);
        }
    }

    /// A function's parameters, and then its block.
    fn visit_function_scope(&mut self, body: &mut FunctionBody<'ast>) {
        self.enter_scope();
        for param in &mut body.params {
            self.declare(&mut param.name);
        }
        // Lua 5.5
        if let Some(name) = body.vararg.as_mut().and_then(|vararg| vararg.name.as_mut()) {
            self.declare(name);
        }
        self.visit_block(&mut body.block);
        self.exit_scope();
    }

    fn visit_scoped_block(&mut self, block: &mut Block<'ast>) {
        self.enter_scope();
        self.visit_block(block);
        self.exit_scope();
    }
}

impl<'ast> VisitMut<'ast> for Renamer<'_, 'ast> {
    fn visit_local_assignment(&mut self, local_assignment: &mut LocalAssignment<'ast>) {
        for expr in &mut local_assignment.exprs {
            self.visit_expr(expr);
        }
        for attributed in &mut local_assignment.names {
            self.visit_attributed_name(attributed);
        }
        for attributed in &mut local_assignment.names {
            self.declare(&mut attributed.name);
        }
    }

    fn visit_local_function(&mut self, local_function: &mut LocalFunction<'ast>) {
        self.visit_signature(&mut local_function.body);
        self.declare(&mut local_function.name);
        self.visit_function_scope(&mut local_function.body);
    }

    fn visit_function_decl(&mut self, function_decl: &mut FunctionDecl<'ast>) {
        if !self.is_applying {
            explicit_self::rewrite(self.allocator, self.bindings, function_decl);
        }
        self.visit_signature(&mut function_decl.body);
        self.visit_function_scope(&mut function_decl.body);
        if let Some(root) = function_decl.name.names.first_mut() {
            self.reference(root);
        }
    }

    fn visit_do_block(&mut self, do_block: &mut DoBlock<'ast>) {
        self.visit_scoped_block(&mut do_block.block);
    }

    fn visit_while_loop(&mut self, while_loop: &mut WhileLoop<'ast>) {
        self.visit_expr(&mut while_loop.condition);
        self.visit_scoped_block(&mut while_loop.block);
    }

    fn visit_repeat_loop(&mut self, repeat_loop: &mut RepeatLoop<'ast>) {
        // The condition sees the block's locals.
        self.enter_scope();
        self.visit_block(&mut repeat_loop.block);
        self.visit_expr(&mut repeat_loop.condition);
        self.exit_scope();
    }

    fn visit_if_stmt(&mut self, if_stmt: &mut IfStatement<'ast>) {
        self.visit_expr(&mut if_stmt.condition);
        self.visit_scoped_block(&mut if_stmt.block);
        for clause in &mut if_stmt.elseif_clauses {
            self.visit_expr(&mut clause.condition);
            self.visit_scoped_block(&mut clause.block);
        }
        if let Some(else_clause) = &mut if_stmt.else_clause {
            self.visit_scoped_block(&mut else_clause.block);
        }
    }

    fn visit_numeric_for(&mut self, numeric_for: &mut NumericFor<'ast>) {
        self.visit_expr(&mut numeric_for.start);
        self.visit_expr(&mut numeric_for.limit);
        if let Some(step) = &mut numeric_for.step {
            self.visit_expr(step);
        }
        // Luau
        if let Some(ty) = &mut numeric_for.type_annotation {
            self.visit_type(ty);
        }
        self.enter_scope();
        self.declare(&mut numeric_for.name);
        self.visit_block(&mut numeric_for.block);
        self.exit_scope();
    }

    fn visit_generic_for(&mut self, generic_for: &mut GenericFor<'ast>) {
        for expr in &mut generic_for.exprs {
            self.visit_expr(expr);
        }
        // Luau: every name binds after the whole list parses.
        for param in &mut generic_for.names {
            if let Some(ty) = &mut param.type_annotation {
                self.visit_type(ty);
            }
        }
        self.enter_scope();
        for param in &mut generic_for.names {
            self.declare(&mut param.name);
        }
        self.visit_block(&mut generic_for.block);
        self.exit_scope();
    }

    // Lua 5.5: as with `local function`, the body sees the name.
    fn visit_global_function(&mut self, global_function: &mut GlobalFunction<'ast>) {
        self.declare(&mut global_function.name);
        self.visit_function_body(&mut global_function.body);
    }

    // Lua 5.5: as with `local`, the initializers see the outer scope.
    fn visit_global_declaration(&mut self, global_declaration: &mut GlobalDeclaration<'ast>) {
        for expr in &mut global_declaration.exprs {
            self.visit_expr(expr);
        }
        for attributed in &mut global_declaration.names {
            self.declare(&mut attributed.name);
        }
    }

    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        if let Expression::Name(name) = expr {
            self.reference(name);
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_var(&mut self, var: &mut Var<'ast>) {
        if let Var::Name(name) = var {
            self.reference(name);
            return;
        }
        self.walk_var(var);
    }

    fn visit_function_body(&mut self, function_body: &mut FunctionBody<'ast>) {
        self.visit_signature(function_body);
        self.visit_function_scope(function_body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                "return(function(l,u)return l,u end)::typeof(function(l)return l end)",
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
