//! Renames every local to the shortest name its scopes allow.
//!
//! One walk resolves each name to a binding with the scoping rules of the
//! target, and records where each binding is live. Bindings that are never
//! live in the same scope share a slot, the most-read slots get the
//! shortest names, and a slot never wears the name of a global read where
//! it's live. The same walk then runs again and writes each binding's name
//! into its declaration and its references, in the order the first one met
//! them.

mod globals;
mod names;

use core::cmp::Reverse;

use luck_allocator::Allocator;
use luck_ast::{Block, Expression, FunctionBody, Name, Statement, Var, VisitorMut};
use luck_token::LuaVersion;
use rustc_hash::FxHashMap;

use self::globals::renamable_globals;
use self::names::{CandidatePool, ScopeClaims};
use crate::binding::{
    IMPLICIT_VARARG_TABLE_NAME, METHOD_RECEIVER_NAME, has_implicit_arg, is_env_binding,
};

/// Renames the locals of `block`, and with `rename_globals`, the globals it
/// assigns before anything reads them. Says whether any name changed.
pub(crate) fn rename<'ast>(
    allocator: &'ast Allocator,
    block: &mut Block<'ast>,
    version: LuaVersion,
    rename_globals: bool,
) -> bool {
    // Opt-in, since renamed globals end up under new `_G` keys.
    let root_globals = if rename_globals { renamable_globals(block, version) } else { Vec::new() };
    let mut walk = RenameWalk::new(version);
    for &name in &root_globals {
        walk.declare_binding(name, BindingKind::Renamable);
    }
    walk.visit_block(block);
    walk.choose_names(allocator);
    walk.next_binding = root_globals.len();
    walk.is_applying = true;
    walk.visit_block(block);
    walk.has_changed
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BindingKind {
    Renamable,
    /// Keeps its name: a function's implicit `self` or 5.1 `arg`, a 5.2+
    /// `_ENV`, or a Luau export.
    Fixed,
    /// Lua 5.5: a `global` declaration. It keeps its name and scopes and
    /// shadows like a local, but its references still reach the real global.
    Global,
}

struct Binding<'ast> {
    name: &'ast str,
    scope: usize,
    kind: BindingKind,
    reference_count: usize,
    /// The declaring scope, the reference scopes, and every scope on the
    /// path between them, in ascending order.
    live_scopes: Vec<usize>,
}

struct Scope {
    parent: Option<usize>,
    /// One past the last scope inside this one. Scope ids are in preorder,
    /// so this one and its descendants are `id..end`.
    end: usize,
}

/// No binding: a global, or a name that keeps its own.
const UNRESOLVED: usize = usize::MAX;

/// The walk that both resolves names and, with `is_applying`, writes the
/// chosen ones.
///
/// Running one walk twice is what keeps the bindings and references the
/// second pass meets in the order the first recorded them.
struct RenameWalk<'ast> {
    version: LuaVersion,
    is_applying: bool,
    scopes: Vec<Scope>,
    current_scope: usize,
    bindings: Vec<Binding<'ast>>,
    /// The bindings in scope where the walk is, in declaration order, and
    /// where each open scope's start.
    open_bindings: Vec<usize>,
    open_scope_starts: Vec<usize>,
    /// Each name's bindings in scope, innermost last.
    name_stacks: FxHashMap<&'ast str, Vec<usize>>,
    /// Each reference's binding, in walk order.
    resolutions: Vec<usize>,
    /// Each name no slot may wear where it's read, and the scopes that read
    /// it, sorted once the walk ends.
    global_reads: FxHashMap<&'ast str, Vec<usize>>,
    new_names: Vec<&'ast str>,
    next_binding: usize,
    next_reference: usize,
    has_changed: bool,
}

impl<'ast> RenameWalk<'ast> {
    fn new(version: LuaVersion) -> Self {
        Self {
            version,
            is_applying: false,
            scopes: vec![Scope { parent: None, end: 1 }],
            current_scope: 0,
            bindings: Vec::new(),
            open_bindings: Vec::new(),
            open_scope_starts: Vec::new(),
            name_stacks: FxHashMap::default(),
            resolutions: Vec::new(),
            global_reads: FxHashMap::default(),
            new_names: Vec::new(),
            next_binding: 0,
            next_reference: 0,
            has_changed: false,
        }
    }

    fn enter_scope(&mut self) {
        if self.is_applying {
            return;
        }
        let id = self.scopes.len();
        self.scopes.push(Scope { parent: Some(self.current_scope), end: id.saturating_add(1) });
        self.current_scope = id;
        self.open_scope_starts.push(self.open_bindings.len());
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
        let start = self.open_scope_starts.pop().unwrap_or(0);
        for binding in self.open_bindings.drain(start..).rev() {
            let Some(name) = self.bindings.get(binding).map(|closing| closing.name) else {
                continue;
            };
            if let Some(stack) = self.name_stacks.get_mut(name) {
                stack.pop();
            }
        }
    }

    /// Declares a binding with no name of its own in the tree. One that
    /// keeps its name counts as read in its own scope, even if nothing reads
    /// it, since a local renamed to that name would otherwise capture every
    /// use of it.
    fn declare_binding(&mut self, name: &'ast str, kind: BindingKind) {
        if self.is_applying {
            self.next_binding = self.next_binding.saturating_add(1);
            return;
        }
        let id = self.bindings.len();
        let scope = self.current_scope;
        self.bindings.push(Binding {
            name,
            scope,
            kind,
            reference_count: 0,
            live_scopes: vec![scope],
        });
        self.open_bindings.push(id);
        self.name_stacks.entry(name).or_default().push(id);
        if kind != BindingKind::Renamable {
            self.resolve(name);
        }
    }

    fn declare(&mut self, name: &mut Name<'ast>, kind: BindingKind) {
        if self.is_applying {
            self.apply(name, self.next_binding);
            self.next_binding = self.next_binding.saturating_add(1);
            return;
        }
        self.declare_binding(name.text, kind);
    }

    fn declare_local(&mut self, name: &mut Name<'ast>, is_exported: bool) {
        let is_fixed = is_exported || is_env_binding(name.text, self.version);
        self.declare(name, if is_fixed { BindingKind::Fixed } else { BindingKind::Renamable });
    }

    fn reference(&mut self, name: &mut Name<'ast>) {
        if self.is_applying {
            let binding = self.resolutions.get(self.next_reference).copied().unwrap_or(UNRESOLVED);
            self.next_reference = self.next_reference.saturating_add(1);
            self.apply(name, binding);
            return;
        }
        let binding = self.resolve(name.text);
        self.resolutions.push(binding);
    }

    fn apply(&mut self, name: &mut Name<'ast>, binding: usize) {
        if let Some(&new_name) = self.new_names.get(binding)
            && name.text != new_name
        {
            name.text = new_name;
            self.has_changed = true;
        }
    }

    /// The binding a read of `name` here reaches, after marking every scope
    /// between the two live for it.
    fn resolve(&mut self, name: &'ast str) -> usize {
        let current_scope = self.current_scope;
        let Self { scopes, bindings, name_stacks, global_reads, .. } = self;
        let innermost = name_stacks.get(name).and_then(|stack| stack.last()).copied();
        let Some((id, binding)) =
            innermost.and_then(|id| bindings.get_mut(id).map(|binding| (id, binding)))
        else {
            global_reads.entry(name).or_default().push(current_scope);
            return UNRESOLVED;
        };
        binding.reference_count = binding.reference_count.saturating_add(1);
        // A scope that's live already brought its ancestors in with it, and
        // the declaring scope is live from the start. A scope not yet live
        // was entered after every one that is, so the new ones go on the
        // end, outermost first.
        let live_count = binding.live_scopes.len();
        let mut next_scope = Some(current_scope);
        while let Some(scope) = next_scope {
            let is_live = binding
                .live_scopes
                .get(..live_count)
                .is_some_and(|live_scopes| live_scopes.binary_search(&scope).is_ok());
            if is_live {
                break;
            }
            binding.live_scopes.push(scope);
            next_scope = scopes.get(scope).and_then(|node| node.parent);
        }
        if let Some(new_scopes) = binding.live_scopes.get_mut(live_count..) {
            new_scopes.reverse();
        }
        // A binding that keeps its name holds that name wherever it's read.
        if binding.kind != BindingKind::Renamable {
            global_reads.entry(name).or_default().push(current_scope);
        }
        id
    }

    /// Gives every renamable binding its slot's name and every other one its
    /// own.
    fn choose_names(&mut self, allocator: &'ast Allocator) {
        // Each list is sorted on its own, so the order between them can't
        // show.
        self.global_reads.values_mut().for_each(|scopes| {
            scopes.sort_unstable();
            scopes.dedup();
        });
        let scope_count = self.scopes.len();
        if let Some(root) = self.scopes.first_mut() {
            root.end = scope_count;
        }
        let slots = self.assign_slots();
        let slot_names = self.name_slots(allocator, &slots);
        self.new_names = self
            .bindings
            .iter()
            .zip(&slots.of_binding)
            .map(|(binding, slot)| {
                slot.and_then(|index| slot_names.get(index).copied()).unwrap_or(binding.name)
            })
            .collect();
    }

    /// Gives each renamable binding the first slot that isn't live in its
    /// scope, in scope preorder.
    fn assign_slots(&self) -> Slots {
        let mut order: Vec<usize> = (0..self.bindings.len()).collect();
        // Scope ids come from one depth-first walk, so they're in preorder:
        // parents before their children. The sort is stable, so a scope's
        // bindings stay in declaration order.
        order.sort_by_key(|&id| self.bindings.get(id).map_or(0, |binding| binding.scope));
        let mut slots =
            Slots { live_scopes: Vec::new(), of_binding: vec![None; self.bindings.len()] };
        let mut live_slots = ScopeClaims::default();
        for id in order {
            let Some(binding) = self.bindings.get(id) else {
                continue;
            };
            if binding.kind != BindingKind::Renamable {
                continue;
            }
            // Reuse the first slot that isn't live in this scope.
            let slot = live_slots.first_free(binding.scope);
            if slot == slots.live_scopes.len() {
                slots.live_scopes.push(Vec::new());
            }
            live_slots.claim(&binding.live_scopes, slot);
            if let Some(live_scopes) = slots.live_scopes.get_mut(slot) {
                live_scopes.extend_from_slice(&binding.live_scopes);
            }
            if let Some(binding_slot) = slots.of_binding.get_mut(id) {
                *binding_slot = Some(slot);
            }
        }
        slots
    }

    fn name_slots(&self, allocator: &'ast Allocator, slots: &Slots) -> Vec<&'ast str> {
        let mut frequencies = vec![0usize; slots.live_scopes.len()];
        for (binding, slot) in self.bindings.iter().zip(&slots.of_binding) {
            if let Some(frequency) = slot.and_then(|index| frequencies.get_mut(index)) {
                *frequency = frequency.saturating_add(binding.reference_count);
            }
        }
        // Most-referenced slots pick first and get the shortest names.
        // Unreferenced slots sort last but still appear in the output, so they
        // go through the same conflict check. Otherwise an unused param could
        // take the name of an upvalue read in the same function body.
        let mut by_frequency: Vec<usize> = (0..slots.live_scopes.len()).collect();
        by_frequency.sort_by_key(|&slot| Reverse(frequencies.get(slot).copied()));
        // A slot may not wear the name of a global read in or below a scope
        // it's live in.
        let reads_global = |scope: usize, name: &str| {
            let end = self.scopes.get(scope).map_or(scope, |node| node.end);
            self.global_reads.get(name).is_some_and(|reading_scopes| {
                let first = reading_scopes.partition_point(|&reading| reading < scope);
                reading_scopes.get(first).is_some_and(|&reading| reading < end)
            })
        };
        let mut pool = CandidatePool::new(allocator, self.version);
        let mut slot_names = vec![""; slots.live_scopes.len()];
        for slot in by_frequency {
            if let (Some(name), Some(live_scopes)) =
                (slot_names.get_mut(slot), slots.live_scopes.get(slot))
            {
                *name = pool.pick(live_scopes, reads_global);
            }
        }
        slot_names
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

    /// A function's parameters and implicit names, and then its block.
    fn visit_function_scope(&mut self, body: &mut FunctionBody<'ast>, is_method: bool) {
        self.enter_scope();
        if is_method {
            self.declare_binding(METHOD_RECEIVER_NAME, BindingKind::Fixed);
        }
        for param in &mut body.params {
            self.declare_local(&mut param.name, false);
        }
        if has_implicit_arg(body, self.version) {
            self.declare_binding(IMPLICIT_VARARG_TABLE_NAME, BindingKind::Fixed);
        }
        // Lua 5.5
        if let Some(name) = body.vararg.as_mut().and_then(|vararg| vararg.name.as_mut()) {
            self.declare_local(name, false);
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

struct Slots {
    /// Each slot's live scopes, the union of its bindings'.
    live_scopes: Vec<Vec<usize>>,
    /// Each binding's slot, `None` for one that keeps its name.
    of_binding: Vec<Option<usize>>,
}

impl<'ast> VisitorMut<'ast> for RenameWalk<'ast> {
    #[expect(clippy::too_many_lines, reason = "one arm per kind of statement")]
    fn visit_stmt(&mut self, stmt: &mut Statement<'ast>) {
        match stmt {
            Statement::LocalAssignment(local) => {
                // Values and annotations before the names: in
                // `local x: typeof(x) = x` both read the outer x.
                for expr in &mut local.exprs {
                    self.visit_expr(expr);
                }
                for attributed in &mut local.names {
                    if let Some(ty) = &mut attributed.type_annotation {
                        self.visit_type(ty);
                    }
                }
                let is_exported = local.kind.is_exported();
                for attributed in &mut local.names {
                    self.declare_local(&mut attributed.name, is_exported);
                }
            }
            Statement::LocalFunction(local_function) => {
                self.visit_signature(&mut local_function.body);
                let is_exported = local_function.kind.is_exported();
                self.declare_local(&mut local_function.name, is_exported);
                self.visit_function_scope(&mut local_function.body, false);
            }
            Statement::FunctionDecl(function_declaration) => {
                self.visit_signature(&mut function_declaration.body);
                let is_method = function_declaration.name.method.is_some();
                self.visit_function_scope(&mut function_declaration.body, is_method);
                if let Some(root) = function_declaration.name.names.first_mut() {
                    self.reference(root);
                }
            }
            Statement::DoBlock(do_block) => self.visit_scoped_block(&mut do_block.block),
            Statement::WhileLoop(while_loop) => {
                self.visit_expr(&mut while_loop.condition);
                self.visit_scoped_block(&mut while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                // The condition sees the block's locals.
                self.enter_scope();
                self.visit_block(&mut repeat_loop.block);
                self.visit_expr(&mut repeat_loop.condition);
                self.exit_scope();
            }
            Statement::IfStatement(if_stmt) => {
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
            Statement::NumericFor(numeric_for) => {
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
                self.declare_local(&mut numeric_for.name, false);
                self.visit_block(&mut numeric_for.block);
                self.exit_scope();
            }
            Statement::GenericFor(generic_for) => {
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
                    self.declare_local(&mut param.name, false);
                }
                self.visit_block(&mut generic_for.block);
                self.exit_scope();
            }
            // Lua 5.5: as with `local function`, the body sees the name.
            Statement::GlobalFunction(global_function) => {
                self.declare(&mut global_function.name, BindingKind::Global);
                self.visit_function_body(&mut global_function.body);
            }
            // Lua 5.5: as with `local`, the initializers see the outer scope.
            Statement::GlobalDeclaration(global_declaration) => {
                for expr in &mut global_declaration.exprs {
                    self.visit_expr(expr);
                }
                for attributed in &mut global_declaration.names {
                    self.declare(&mut attributed.name, BindingKind::Global);
                }
            }
            // These bind nothing themselves. The walk still reaches the
            // functions they hold, Luau `type function` bodies and `typeof`
            // included.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_) => self.walk_stmt(stmt),
        }
    }

    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        if let Expression::Name(name) = expr {
            self.reference(name);
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &mut Var<'ast>) {
        if let Var::Name(name) = target {
            self.reference(name);
            return;
        }
        self.walk_assignment_target(target);
    }

    fn visit_function_body(&mut self, body: &mut FunctionBody<'ast>) {
        self.visit_signature(body);
        self.visit_function_scope(body, false);
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use super::*;
    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const RENAMING: TransformConfig =
        TransformConfig { rename_locals: true, rename_globals: true, ..NO_PASSES };

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    fn apply_version(source: &str, version: LuaVersion) -> String {
        minify_with(source, version, &RENAMING)
    }

    #[test]
    fn many_locals_live_at_once_rename_without_rescanning_slots_or_names() {
        // Each local needs a slot and a name of its own. Looking either up
        // from the first one every time takes minutes at this size.
        let mut source = String::new();
        for local in 0..120_000u32 {
            let _unused = writeln!(source, "local a{local} = f({local})");
        }
        source.push_str("return a0, a119999");
        let result = apply(&source);
        // The two locals read twice take the two shortest names.
        assert!(
            result.starts_with("local l=f(0)local c=f(1)local k=f(2)"),
            "{:?}",
            result.get(..60)
        );
        assert!(result.ends_with("return l,u"), "the output ends in the return");
    }

    #[test]
    fn many_sibling_scopes_share_one_name() {
        let mut source = String::new();
        for scope in 0..20_000u32 {
            let _unused = writeln!(source, "do local x{scope} = f() g(x{scope}) end");
        }
        let result = apply(&source);
        assert_eq!(result, "do local l=f()g(l)end ".repeat(20_000).trim_end());
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

    #[test]
    fn global_read_via_typecast_is_not_assign_first() {
        // `g` is read inside a type cast before it's assigned, so it's a real
        // global and must keep its name.
        let result = apply_luau(
            "\
h = (g :: any)
g = 1
print(h, g)
",
        );
        assert!(result.contains('g'), "global 'g' read via typecast must be preserved: {result}");
    }

    #[test]
    fn global_read_via_compound_index_is_not_assign_first() {
        // `g` is read as the index of a compound target before it's assigned,
        // so it's a real global and must keep its name, at any nesting depth.
        for (source, expected) in [
            (
                "\
t[g] += 1
g = 1
print(t, g)
",
                "t[g]+=1 g=1 print(t,g)",
            ),
            (
                "\
do t[g] += 1 end
g = 1
print(t, g)
",
                "do t[g]+=1 end g=1 print(t,g)",
            ),
        ] {
            let result = apply_luau(source);
            assert_eq!(result, expected);
        }
    }

    fn assert_luau_renames(cases: &[(&str, &str)]) {
        for &(source, expected) in cases {
            let result = apply_luau(source);
            assert_eq!(result, expected, "source: {source}");
        }
    }

    #[test]
    fn global_read_via_assignment_target_is_not_assign_first() {
        // A target's prefix and index are reads at any depth of the target
        // chain, so `g` is a real global and must keep its name.
        assert_luau_renames(&[
            (
                "\
t[g] = 1
g = 1
print(t, g)
",
                "t[g]=1 g=1 print(t,g)",
            ),
            (
                "\
g[1] = 2
g = 1
print(g)
",
                "g[1]=2 g=1 print(g)",
            ),
            (
                "\
g.a.b = 1
g = 1
print(g)
",
                "g.a.b=1 g=1 print(g)",
            ),
            (
                "\
g[1].b = 1
g = 1
print(g)
",
                "g[1].b=1 g=1 print(g)",
            ),
            (
                "\
t[g].b = 1
g = 1
print(t, g)
",
                "t[g].b=1 g=1 print(t,g)",
            ),
            (
                "\
f(g).x = 1
g = 1
print(g)
",
                "f(g).x=1 g=1 print(g)",
            ),
            (
                "\
(g).x = 1
g = 1
print(g)
",
                "(g).x=1 g=1 print(g)",
            ),
            (
                "\
t[g], g = 1, 2
print(t, g)
",
                "t[g],g=1,2 print(t,g)",
            ),
        ]);
    }

    #[test]
    fn global_read_by_later_target_is_not_assign_first() {
        // Every target's prefix and index is evaluated before any store, so `g`
        // is read here even though its own target comes first.
        assert_luau_renames(&[
            (
                "\
g, t[g] = 1, 2
print(t, g)
",
                "g,t[g]=1,2 print(t,g)",
            ),
            (
                "\
g, g[1] = 1, 2
print(g)
",
                "g,g[1]=1,2 print(g)",
            ),
            (
                "\
g, g.x = 1, 2
print(g)
",
                "g,g.x=1,2 print(g)",
            ),
        ]);
    }

    #[test]
    fn global_read_via_nested_return_is_not_assign_first() {
        // A return sits outside its block's statement list, and an early one
        // reads `g` before the assignment that follows the block.
        assert_luau_renames(&[
            (
                "\
do return g end
g = 1
print(g)
",
                "do return g end g=1 print(g)",
            ),
            (
                "\
if c then return g end
g = 1
print(g)
",
                "if c then return g end g=1 print(g)",
            ),
            (
                "\
if c then print(1) elseif d then return g end
g = 1
print(g)
",
                "if c then print(1)elseif d then return g end g=1 print(g)",
            ),
            (
                "\
if c then print(1) else return g end
g = 1
print(g)
",
                "if c then print(1)else return g end g=1 print(g)",
            ),
            (
                "\
while c do return g end
g = 1
print(g)
",
                "while c do return g end g=1 print(g)",
            ),
            (
                "\
repeat return g until c
g = 1
print(g)
",
                "repeat return g until c g=1 print(g)",
            ),
            (
                "\
for i = 1, 2 do return g end
g = 1
print(g)
",
                "for l=1,2 do return g end g=1 print(g)",
            ),
            (
                "\
for k in x do return g end
g = 1
print(g)
",
                "for l in x do return g end g=1 print(g)",
            ),
        ]);
    }

    #[test]
    fn global_read_via_function_name_is_not_assign_first() {
        // `function g.f()` and `function g:f()` read `g` to store into it.
        assert_luau_renames(&[
            (
                "\
function g.x() end
g = 1
print(g)
",
                "function g.x()end g=1 print(g)",
            ),
            (
                "\
function g:m() end
g = 1
print(g)
",
                "function g:m()end g=1 print(g)",
            ),
            (
                "\
function g.a.b() end
g = 1
print(g)
",
                "function g.a.b()end g=1 print(g)",
            ),
            (
                "\
do function g.x() end end
g = 1
print(g)
",
                "do function g.x()end end g=1 print(g)",
            ),
        ]);
    }

    #[test]
    fn global_assigned_before_any_read_stays_assign_first() {
        // Later reads, a plain `function g()` (which only assigns), and a read
        // deferred into a function body all leave `g` assign-first, so it's
        // still renamed. In the last case, `h` stays assign-first even though
        // a target beside it reads `g`.
        assert_luau_renames(&[
            (
                "\
g = 1
t[g] = 2
print(t, g)
",
                "l=1 t[l]=2 print(t,l)",
            ),
            (
                "\
g = 1
g.a.b = 2
print(g)
",
                "l=1 l.a.b=2 print(l)",
            ),
            (
                "\
g = 1
function g.x() end
print(g)
",
                "l=1 function l.x()end print(l)",
            ),
            (
                "\
do function g() end end
g = 1
print(g)
",
                "do function l()end end l=1 print(l)",
            ),
            (
                "\
h = function() return g end
g = 1
print(h, g)
",
                "u=function()return l end l=1 print(u,l)",
            ),
            (
                "\
h, t[g] = 1, 2
print(h, t)
",
                "l,t[g]=1,2 print(l,t)",
            ),
        ]);
    }

    // Same reads as the assign-first check, at the same depths.
    const READ_BEFORE_DEFINITION_CASES: &[(&str, &str)] = &[
        (
            "\
print(g)
function g() end
",
            "print(g)function g()end",
        ),
        (
            "\
x = g
function g() end
",
            "l=g function g()end",
        ),
        (
            "\
t[g] = 1
function g() end
",
            "t[g]=1 function g()end",
        ),
        (
            "\
g.x = 1
function g() end
",
            "g.x=1 function g()end",
        ),
        (
            "\
g[1] = 1
function g() end
",
            "g[1]=1 function g()end",
        ),
        (
            "\
g, t[g] = 1, 2
function g() end
",
            "g,t[g]=1,2 function g()end",
        ),
        (
            "\
g += 1
function g() end
",
            "g+=1 function g()end",
        ),
        (
            "\
function g.x() end
function g() end
",
            "function g.x()end function g()end",
        ),
        (
            "\
function g:m() end
function g() end
",
            "function g:m()end function g()end",
        ),
        (
            "\
if c then return g end
function g() end
",
            "if c then return g end function g()end",
        ),
        (
            "\
do print(g) end
function g() end
",
            "do print(g)end function g()end",
        ),
        (
            "\
print(g)
function g() end
function g() end
",
            "print(g)function g()end function g()end",
        ),
        (
            "\
print(g)
function g() end
g = 1
print(g)
",
            "print(g)function g()end g=1 print(g)",
        ),
        (
            "\
print(g)
do local g = 1 end
function g() end
",
            "print(g)do local l=1 end function g()end",
        ),
    ];

    #[test]
    fn global_read_before_function_definition_is_not_renamed() {
        assert_luau_renames(READ_BEFORE_DEFINITION_CASES);
    }

    #[test]
    fn nested_read_before_the_local_it_shares_a_name_with_is_a_global_read() {
        assert_luau_renames(&[
            (
                "\
do print(g)
local g = 1 end
function g() end
",
                "do print(g)local l=1 end function g()end",
            ),
            (
                "\
do local g = g end
function g() end
",
                "do local l=g end function g()end",
            ),
            (
                "\
do do local g = f() end
print(g) end
function g() end
",
                "do do local l=f()end print(g)end function g()end",
            ),
            (
                "\
for g = g, 2 do end
function g() end
",
                "for l=g,2 do end function g()end",
            ),
            (
                "\
for g in g do end
function g() end
",
                "for l in g do end function g()end",
            ),
            (
                "\
repeat print(g)
local g = f() until g
function g() end
",
                "repeat print(g)local l=f()until l function g()end",
            ),
        ]);
    }

    #[test]
    fn read_of_a_nested_local_is_not_a_read_of_the_global() {
        assert_luau_renames(&[
            (
                "\
do local g = f()
print(g) end
function g() end
",
                "do local l=f()print(l)end function l()end",
            ),
            (
                "\
do local function g() end
g() end
function g() end
",
                "do local function l()end l()end function l()end",
            ),
            (
                "\
for g = 1, 2 do print(g) end
function g() end
",
                "for l=1,2 do print(l)end function l()end",
            ),
            (
                "\
for g in x do print(g) end
function g() end
",
                "for l in x do print(l)end function l()end",
            ),
            (
                "\
repeat local g = f() until g
function g() end
",
                "repeat local l=f()until l function l()end",
            ),
        ]);
    }

    #[test]
    fn assignment_before_any_read_keeps_a_function_global() {
        // The tie between `a` and `g` goes to the earlier binding, so moving
        // `g` out of the function globals would swap their names.
        assert_luau_renames(&[
            (
                "\
g = 1
do local g = f() end
print(g)
function g() end
",
                "l=1 do local l=f()end print(l)function l()end",
            ),
            (
                "\
a = 1
print(a)
print(a)
g = 1
print(g)
function g() end
",
                "u=1 print(u)print(u)l=1 print(l)function l()end",
            ),
        ]);
    }

    #[test]
    fn function_defined_before_any_read_stays_renamed() {
        // A read after the definition and a function body's deferred read both
        // leave `g` renamed, and so does an assignment that got there first.
        assert_luau_renames(&[
            (
                "\
function g() end
print(g)
",
                "function l()end print(l)",
            ),
            ("function g() return g end\n", "function l()return l end"),
            (
                "\
function g() end
print(g)
function g() end
",
                "function l()end print(l)function l()end",
            ),
            (
                "\
h = function() return g end
function g() end
",
                "u=function()return l end function l()end",
            ),
            (
                "\
function f() return g end
f()
function g() end
",
                "function l()return u end l()function u()end",
            ),
            (
                "\
g = 1
function g() end
print(g)
",
                "l=1 function l()end print(l)",
            ),
            (
                "\
g = 1
print(g)
function g() end
",
                "l=1 print(l)function l()end",
            ),
            (
                "\
do local g = 1 end
function g() end
",
                "do local l=1 end function l()end",
            ),
        ]);
    }

    #[test]
    fn deferred_global_read_is_left_as_it_was() {
        let result = apply(
            "\
function f() return g end
f()
g = 1
",
        );
        assert_eq!(result, "function l()return u end l()u=1");
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

    fn assert_lua55_renames(cases: &[(&str, &str)]) {
        for &(source, expected) in cases {
            let result = apply_version(source, LuaVersion::Lua55);
            assert_eq!(result, expected, "source: {source}");
        }
    }

    #[test]
    fn globally_declared_name_is_not_renamed() {
        assert_lua55_renames(&[
            (
                "\
global g
g = 1
",
                "global g g=1",
            ),
            (
                "\
g = 1
global g
",
                "g=1 global g",
            ),
            (
                "\
g = 1
global g
g = 2
",
                "g=1 global g g=2",
            ),
            (
                "\
global g = 1
g = 2
",
                "global g=1 g=2",
            ),
            (
                "\
global a, g
g = 1
a = 2
",
                "global a,g g=1 a=2",
            ),
            ("global <const> g = 1\n", "global g<const> =1"),
            (
                "\
function g() end
global g
",
                "function g()end global g",
            ),
            (
                "\
global g
function g() end
",
                "global g function g()end",
            ),
        ]);
    }

    #[test]
    fn name_of_global_function_is_not_renamed() {
        assert_lua55_renames(&[
            (
                "\
g = 1
global function g() end
",
                "g=1 global function g()end",
            ),
            (
                "\
function g() end
global function g() end
",
                "function g()end global function g()end",
            ),
            (
                "\
global function g() return g end
g()
",
                "global function g()return g end g()",
            ),
        ]);
    }

    #[test]
    fn global_declaration_pins_its_name_from_any_depth() {
        assert_lua55_renames(&[
            (
                "\
g = 1
do global g
g = 2 end
",
                "g=1 do global g g=2 end",
            ),
            (
                "\
g = 1
if c then global g end
",
                "g=1 if c then global g end",
            ),
            (
                "\
g = 1
function f() global g
g = 2 end
",
                "g=1 function l()global g g=2 end",
            ),
            (
                "\
function g() end
local function f() global function g() end end
return f
",
                "function g()end local function l()global function g()end end return l",
            ),
        ]);
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
    fn collective_global_declaration_leaves_every_name_free() {
        // `global *` declares no name of its own, so renamed globals stay
        // legal.
        assert_lua55_renames(&[
            (
                "\
global *
g = 1
print(g)
",
                "global*l=1 print(l)",
            ),
            (
                "\
local g = f()
global *
g = 1
",
                "local l=f()global*l=1",
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
