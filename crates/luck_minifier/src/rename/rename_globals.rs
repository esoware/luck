//! The globals `rename_globals` renames: each one a file defines at its top
//! level before anything there reads it.
//!
//! A `do` block at the top level runs where it stands, so its statements
//! count as the top level's, which keeps the answer the same once a `do`
//! block that scopes nothing is unwrapped.

use luck_ast::{Assignment, Block, Expression, FunctionBody, Name, Statement, Type, Var, Visit};
use luck_data_structures::BitSet;
use luck_semantic::{NameId, Scoping};

/// The globals to rename, in the order the renamer declares them as root
/// bindings: each top-level `function g()` name, then each name a top-level
/// assignment defines.
pub(super) fn renamable_globals(block: &Block<'_>, scoping: &Scoping) -> Vec<NameId> {
    let mut top_level = Vec::new();
    flatten(block, &mut top_level);
    let mut renamable = function_globals(&top_level, scoping);
    let mut is_renamable = BitSet::new(scoping.names().len());
    for name in &renamable {
        is_renamable.insert(name.index());
    }

    let mut reads = Reads::new(scoping);
    for &stmt in &top_level {
        let Statement::Assignment(assignment) = stmt else {
            reads.visit_stmt(stmt);
            continue;
        };
        reads.read_assignment(assignment);
        for target in &assignment.targets {
            if let Some(name) = target_global(target, scoping)
                && !is_renamable.contains(name.index())
                && !reads.has(name)
            {
                is_renamable.insert(name.index());
                renamable.push(name);
            }
        }
    }
    renamable
}

/// The global `name` reads or writes, by its scoping.
fn global_of(name: &Name<'_>, scoping: &Scoping) -> Option<NameId> {
    let reference = scoping.reference(scoping.reference_of(name)?);
    reference.resolved.is_none().then_some(reference.name)
}

/// The top level's statements, with each top-level `do` block's in its
/// place.
fn flatten<'tree, 'ast>(block: &'tree Block<'ast>, top_level: &mut Vec<&'tree Statement<'ast>>) {
    for stmt in &block.stmts {
        if let Statement::DoBlock(do_block) = stmt {
            flatten(&do_block.block, top_level);
        } else {
            top_level.push(stmt);
        }
    }
}

fn target_global(target: &Var<'_>, scoping: &Scoping) -> Option<NameId> {
    if let Var::Name(name) = target { global_of(name, scoping) } else { None }
}

/// The name of each top-level `function g()` nothing at the top level reads
/// before it, unless an assignment to `g` came first, once each.
fn function_globals(top_level: &[&Statement<'_>], scoping: &Scoping) -> Vec<NameId> {
    let name_count = scoping.names().len();
    let mut reads = Reads::new(scoping);
    let mut is_assigned_first = BitSet::new(name_count);
    let mut is_taken = BitSet::new(name_count);
    let mut function_globals = Vec::new();
    for &stmt in top_level {
        match stmt {
            Statement::Assignment(assignment) => {
                reads.read_assignment(assignment);
                for target in &assignment.targets {
                    if let Some(name) = target_global(target, scoping)
                        && !reads.has(name)
                    {
                        is_assigned_first.insert(name.index());
                    }
                }
            }
            Statement::FunctionDecl(function_declaration) => {
                let defined = match &*function_declaration.name.names {
                    [root] if function_declaration.name.method.is_none() => {
                        global_of(root, scoping)
                    }
                    _ => None,
                };
                let Some(name) = defined else {
                    reads.visit_stmt(stmt);
                    continue;
                };
                let is_read_first = reads.has(name) && !is_assigned_first.contains(name.index());
                if !is_read_first && !is_taken.contains(name.index()) {
                    is_taken.insert(name.index());
                    function_globals.push(name);
                }
            }
            Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::LocalFunction(_)
            | Statement::LocalAssignment(_)
            | Statement::Return(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => reads.visit_stmt(stmt),
        }
    }
    function_globals
}

/// The globals statements read, outside function bodies and types.
struct Reads<'scoping> {
    scoping: &'scoping Scoping,
    is_read: BitSet,
}

impl<'scoping> Reads<'scoping> {
    fn new(scoping: &'scoping Scoping) -> Self {
        Self { scoping, is_read: BitSet::new(scoping.names().len()) }
    }

    fn has(&self, name: NameId) -> bool {
        self.is_read.contains(name.index())
    }

    fn read(&mut self, name: &Name<'_>) {
        if let Some(global) = global_of(name, self.scoping) {
            self.is_read.insert(global.index());
        }
    }

    /// Every target's prefix and index is evaluated before any store, so
    /// `g, t[g] = 1, 2` reads `g` even though its own target comes first.
    fn read_assignment(&mut self, assignment: &Assignment<'_>) {
        for expr in &assignment.values {
            self.visit_expr(expr);
        }
        for target in &assignment.targets {
            if !matches!(target, Var::Name(_)) {
                self.visit_var(target);
            }
        }
    }
}

impl<'tree> Visit<'tree> for Reads<'_> {
    fn visit_stmt(&mut self, stmt: &'tree Statement<'tree>) {
        // `function g.f()` and `function g:f()` read `g` to store into it. A
        // plain `function g()` only assigns it.
        if let Statement::FunctionDecl(function_declaration) = stmt {
            if let [root, nested @ ..] = &*function_declaration.name.names
                && (!nested.is_empty() || function_declaration.name.method.is_some())
            {
                self.read(root);
            }
            return;
        }
        self.walk_stmt(stmt);
    }

    fn visit_expr(&mut self, expr: &'tree Expression<'tree>) {
        if let Expression::Name(name) = expr {
            self.read(name);
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_var(&mut self, var: &'tree Var<'tree>) {
        if let Var::Name(name) = var {
            self.read(name);
            return;
        }
        self.walk_var(var);
    }

    // A function body's reads belong to its own scope.
    fn visit_function_body(&mut self, _body: &'tree FunctionBody<'tree>) {}

    // Types never run, so they read nothing.
    fn visit_type(&mut self, _ty: &'tree Type<'tree>) {}
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};

    use luck_allocator::Allocator;

    use super::renamable_globals;
    use crate::binding::analyze;
    use crate::fixtures::{RENAMING, assert_renames, minify_with};
    use crate::transform_config::TransformConfig;

    fn apply(source: &str) -> String {
        minify_with(source, &RENAMING)
    }

    #[test]
    fn global_read_via_typecast_is_not_assign_first() {
        // `g` is read inside a type cast before it's assigned, so it's a real
        // global and must keep its name.
        let result = apply(
            "\
h = (g :: any)
g = 1
print(h, g)
",
        );
        assert!(result.contains('g'), "global 'g' read via typecast must be preserved: {result}");
    }

    #[test]
    fn a_global_defined_in_a_top_level_do_block_is_renamed_as_at_the_top_level() {
        assert_renames(&[
            ("do function g() end end g()\n", "do function e()end end e()"),
            (
                "do local x = f() function g() return x end end g()\n",
                "do local t=f()function e()return t end end e()",
            ),
            ("do g = 1 end print(g)\n", "do e=1 end print(e)"),
        ]);
        // Unwrapping the block that scopes nothing changes nothing about it.
        let config = TransformConfig { rename_globals: true, ..TransformConfig::default() };
        assert_eq!(minify_with("do function g() end end g()\n", &config), "function e()end e()");
    }

    #[test]
    fn a_global_defined_in_a_function_body_keeps_its_name() {
        assert_renames(&[(
            "(function() function g() end end)() g()\n",
            "(function()function g()end end)()g()",
        )]);
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
            let result = apply(source);
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn global_read_via_assignment_target_is_not_assign_first() {
        // A target's prefix and index are reads at any depth of the target
        // chain, so `g` is a real global and must keep its name.
        assert_renames(&[
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
                "g.x=1 g=1 print(g)",
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
        assert_renames(&[
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
        assert_renames(&[
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
                "for e=1,2 do return g end g=1 print(g)",
            ),
            (
                "\
for k in x do return g end
g = 1
print(g)
",
                "for e in x do return g end g=1 print(g)",
            ),
        ]);
    }

    #[test]
    fn global_read_via_function_name_is_not_assign_first() {
        // `function g.f()` and `function g:f()` read `g` to store into it.
        assert_renames(&[
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
        assert_renames(&[
            (
                "\
g = 1
t[g] = 2
print(t, g)
",
                "e=1 t[e]=2 print(t,e)",
            ),
            (
                "\
g = 1
g.a.b = 2
print(g)
",
                "e=1 e.a.b=2 print(e)",
            ),
            (
                "\
g = 1
function g.x() end
print(g)
",
                "e=1 function e.x()end print(e)",
            ),
            (
                "\
do function g() end end
g = 1
print(g)
",
                "do function e()end end e=1 print(e)",
            ),
            (
                "\
h = function() return g end
g = 1
print(h, g)
",
                "t=function()return e end e=1 print(t,e)",
            ),
            (
                "\
h, t[g] = 1, 2
print(h, t)
",
                "e,t[g]=1,2 print(e,t)",
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
            "e=g function g()end",
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
            "print(g)do local e=1 end function g()end",
        ),
    ];

    #[test]
    fn global_read_before_function_definition_is_not_renamed() {
        assert_renames(READ_BEFORE_DEFINITION_CASES);
    }

    #[test]
    fn nested_read_before_the_local_it_shares_a_name_with_is_a_global_read() {
        assert_renames(&[
            (
                "\
do print(g)
local g = 1 end
function g() end
",
                "do print(g)local e=1 end function g()end",
            ),
            (
                "\
do local g = g end
function g() end
",
                "do local e=g end function g()end",
            ),
            (
                "\
do do local g = f() end
print(g) end
function g() end
",
                "do do local e=f()end print(g)end function g()end",
            ),
            (
                "\
for g = g, 2 do end
function g() end
",
                "for e=g,2 do end function g()end",
            ),
            (
                "\
for g in g do end
function g() end
",
                "for e in g do end function g()end",
            ),
            (
                "\
repeat print(g)
local g = f() until g
function g() end
",
                "repeat print(g)local e=f()until e function g()end",
            ),
        ]);
    }

    #[test]
    fn read_of_a_nested_local_is_not_a_read_of_the_global() {
        assert_renames(&[
            (
                "\
do local g = f()
print(g) end
function g() end
",
                "do local e=f()print(e)end function e()end",
            ),
            (
                "\
do local function g() end
g() end
function g() end
",
                "do local function e()end e()end function e()end",
            ),
            (
                "\
for g = 1, 2 do print(g) end
function g() end
",
                "for e=1,2 do print(e)end function e()end",
            ),
            (
                "\
for g in x do print(g) end
function g() end
",
                "for e in x do print(e)end function e()end",
            ),
            (
                "\
repeat local g = f() until g
function g() end
",
                "repeat local e=f()until e function e()end",
            ),
        ]);
    }

    #[test]
    fn assignment_before_any_read_keeps_a_function_global() {
        // The tie between `a` and `g` goes to the earlier binding, so moving
        // `g` out of the function globals would swap their names.
        assert_renames(&[
            (
                "\
g = 1
do local g = f() end
print(g)
function g() end
",
                "e=1 do local e=f()end print(e)function e()end",
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
                "t=1 print(t)print(t)e=1 print(e)function e()end",
            ),
        ]);
    }

    #[test]
    fn function_defined_before_any_read_stays_renamed() {
        // A read after the definition and a function body's deferred read both
        // leave `g` renamed, and so does an assignment that got there first.
        assert_renames(&[
            (
                "\
function g() end
print(g)
",
                "function e()end print(e)",
            ),
            ("function g() return g end\n", "function e()return e end"),
            (
                "\
function g() end
print(g)
function g() end
",
                "function e()end print(e)function e()end",
            ),
            (
                "\
h = function() return g end
function g() end
",
                "t=function()return e end function e()end",
            ),
            (
                "\
function f() return g end
f()
function g() end
",
                "function e()return t end e()function t()end",
            ),
            (
                "\
g = 1
function g() end
print(g)
",
                "e=1 function e()end print(e)",
            ),
            (
                "\
g = 1
print(g)
function g() end
",
                "e=1 print(e)function e()end",
            ),
            (
                "\
do local g = 1 end
function g() end
",
                "do local e=1 end function e()end",
            ),
        ]);
    }

    #[test]
    fn many_function_globals_and_assignments_are_found_without_rescanning_them() {
        let count = 300_000;
        let mut source = String::new();
        for i in 0..count {
            writeln!(source, "function f{i}() end").unwrap();
        }
        for i in 0..count {
            writeln!(source, "g{i} = f{i}").unwrap();
        }
        let allocator = Allocator::new();
        let parsed = luck_parser::parse(&allocator, source.as_bytes());
        let scoping = analyze(&parsed.chunk, parsed.counts, None);
        let started = Instant::now();
        let renamable = renamable_globals(&parsed.chunk, &scoping);
        let elapsed = started.elapsed();
        assert_eq!(renamable.len(), 2 * count);
        // Looking each assigned global up among the function globals one by
        // one takes several seconds at this size.
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
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
        assert_eq!(result, "function e()return t end e()t=1");
    }
}
