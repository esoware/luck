//! The globals `rename_globals` renames: each one a file defines at its top
//! level before anything there reads it, and declares nowhere.

use luck_ast::{Assignment, Block, Expression, FunctionBody, Statement, Type, Var, VisitorMut};
use luck_token::LuaVersion;
use rustc_hash::FxHashSet;

use crate::binding::is_env_binding;

/// The globals to rename, in the order the renamer declares them as root
/// bindings: each top-level `function g()` name, then each name a top-level
/// assignment defines.
pub(crate) fn renamable_globals<'ast>(
    block: &mut Block<'ast>,
    version: LuaVersion,
) -> Vec<&'ast str> {
    let mut declarations = DeclaredGlobals::default();
    declarations.visit_block(block);
    let declared_globals = declarations.0;
    let function_globals = toplevel_function_globals(block, &declared_globals, version);
    let assign_first_globals =
        assign_first_globals(block, &function_globals, &declared_globals, version);

    let mut names = Vec::new();
    let mut seen_names = FxHashSet::default();
    for stmt in &block.stmts {
        if let Statement::FunctionDecl(function_declaration) = stmt
            && let [name] = &*function_declaration.name.names
            && function_declaration.name.method.is_none()
            && function_globals.contains(name.text)
            && seen_names.insert(name.text)
        {
            names.push(name.text);
        }
    }
    for stmt in &block.stmts {
        if let Statement::Assignment(assignment) = stmt {
            for target in &assignment.targets {
                if let Var::Name(name) = target
                    && assign_first_globals.contains(name.text)
                    && seen_names.insert(name.text)
                {
                    names.push(name.text);
                }
            }
        }
    }
    names
}

/// A name declared anywhere is the same variable everywhere, so one
/// declaration keeps the whole file from renaming it.
#[derive(Default)]
struct DeclaredGlobals<'ast>(FxHashSet<&'ast str>);

impl<'ast> VisitorMut<'ast> for DeclaredGlobals<'ast> {
    fn visit_stmt(&mut self, stmt: &mut Statement<'ast>) {
        match stmt {
            // Lua 5.5
            Statement::GlobalDeclaration(global_declaration) => {
                self.0
                    .extend(global_declaration.names.iter().map(|attributed| attributed.name.text));
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.0.insert(global_function.name.text);
            }
            // `global *` names nothing, and the rest declare no global.
            // `walk_stmt` below still descends into nested blocks.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
    }
}

fn toplevel_function_globals<'ast>(
    block: &mut Block<'ast>,
    declared_globals: &FxHashSet<&'ast str>,
    version: LuaVersion,
) -> FxHashSet<&'ast str> {
    let mut top_locals: FxHashSet<&'ast str> = FxHashSet::default();
    for stmt in &block.stmts {
        match stmt {
            Statement::LocalAssignment(local) => {
                top_locals.extend(local.names.iter().map(|attributed| attributed.name.text));
            }
            Statement::LocalFunction(local_function) => {
                top_locals.insert(local_function.name.text);
            }
            // Nothing else binds a local at this level.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }

    let mut reads = NameReads::new(&top_locals);
    let mut assigned_first_names: FxHashSet<&'ast str> = FxHashSet::default();
    let mut function_globals = FxHashSet::default();
    for stmt in &mut block.stmts {
        if let Statement::Assignment(assignment) = stmt {
            reads.visit_assignment_reads(assignment);
            for target in &assignment.targets {
                if let Var::Name(name) = target
                    && !reads.names.contains(name.text)
                {
                    assigned_first_names.insert(name.text);
                }
            }
        } else if let Statement::FunctionDecl(function_declaration) = stmt
            && let [function_name] = &*function_declaration.name.names
            && function_declaration.name.method.is_none()
        {
            let name = function_name.text;
            let is_read_first = reads.names.contains(name) && !assigned_first_names.contains(name);
            if !top_locals.contains(name)
                && !is_read_first
                && !declared_globals.contains(name)
                && !is_env_binding(name, version)
            {
                function_globals.insert(name);
            }
        } else {
            reads.visit_stmt(stmt);
        }
    }
    function_globals
}

/// A name assigned at the top level before anything there reads it, and
/// while no top-level local of that name is in scope.
fn assign_first_globals<'ast>(
    block: &mut Block<'ast>,
    function_globals: &FxHashSet<&'ast str>,
    declared_globals: &FxHashSet<&'ast str>,
    version: LuaVersion,
) -> FxHashSet<&'ast str> {
    let no_locals = FxHashSet::default();
    let mut reads = NameReads::new(&no_locals);
    let mut assign_first_globals: FxHashSet<&'ast str> = FxHashSet::default();
    for stmt in &mut block.stmts {
        if let Statement::Assignment(assignment) = stmt {
            reads.visit_assignment_reads(assignment);
            for target in &assignment.targets {
                if let Var::Name(target_name) = target {
                    let name = target_name.text;
                    if !reads.is_local(name)
                        && !function_globals.contains(name)
                        && !reads.names.contains(name)
                        && !declared_globals.contains(name)
                        && !is_env_binding(name, version)
                    {
                        assign_first_globals.insert(name);
                    }
                }
            }
            continue;
        }
        // Everything else only contributes reads, and a top-level local
        // makes its name local from here on.
        reads.visit_stmt(stmt);
        reads.hide_declared(stmt);
    }
    assign_first_globals
}

/// The names statements read as globals: each name outside `locals` and
/// outside the locals the enclosing nested blocks have declared so far.
struct NameReads<'locals, 'ast> {
    locals: &'locals FxHashSet<&'ast str>,
    hidden_names: FxHashSet<&'ast str>,
    /// What `hidden_names` gained, in order, so each scope can drop its own on exit.
    hidden_log: Vec<&'ast str>,
    names: FxHashSet<&'ast str>,
}

impl<'locals, 'ast> NameReads<'locals, 'ast> {
    fn new(locals: &'locals FxHashSet<&'ast str>) -> Self {
        Self {
            locals,
            hidden_names: FxHashSet::default(),
            hidden_log: Vec::new(),
            names: FxHashSet::default(),
        }
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals.contains(name) || self.hidden_names.contains(name)
    }

    fn read(&mut self, name: &'ast str) {
        if !self.is_local(name) {
            self.names.insert(name);
        }
    }

    fn hide(&mut self, name: &'ast str) {
        if !self.is_local(name) {
            self.hidden_names.insert(name);
            self.hidden_log.push(name);
        }
    }

    fn unhide_from(&mut self, mark: usize) {
        for name in self.hidden_log.drain(mark..) {
            self.hidden_names.remove(name);
        }
    }

    fn hide_declared(&mut self, stmt: &Statement<'ast>) {
        match stmt {
            Statement::LocalAssignment(local) => {
                for attributed in &local.names {
                    self.hide(attributed.name.text);
                }
            }
            Statement::LocalFunction(local_function) => self.hide(local_function.name.text),
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }

    /// A block's own locals don't count as reads. Slot reuse gives nested
    /// locals the names of renamed globals, so on every later round they'd
    /// otherwise look like reads of those globals.
    fn visit_scope(&mut self, block: &mut Block<'ast>) -> usize {
        let mark = self.hidden_log.len();
        for stmt in &mut block.stmts {
            self.visit_stmt(stmt);
            self.hide_declared(stmt);
        }
        if let Some(last) = &mut block.last_stmt {
            self.visit_last_stmt(last);
        }
        mark
    }

    /// Every target's prefix and index is evaluated before any store, so
    /// `g, t[g] = 1, 2` reads `g` even though its own target comes first.
    fn visit_assignment_reads(&mut self, assignment: &mut Assignment<'ast>) {
        for expr in &mut assignment.values {
            self.visit_expr(expr);
        }
        for target in &mut assignment.targets {
            if !matches!(target, Var::Name(_)) {
                self.visit_assignment_target(target);
            }
        }
    }
}

impl<'ast> VisitorMut<'ast> for NameReads<'_, 'ast> {
    fn visit_block(&mut self, block: &mut Block<'ast>) {
        let mark = self.visit_scope(block);
        self.unhide_from(mark);
    }

    fn visit_stmt(&mut self, stmt: &mut Statement<'ast>) {
        match stmt {
            Statement::RepeatLoop(repeat_loop) => {
                // The condition sees the block's locals.
                let mark = self.visit_scope(&mut repeat_loop.block);
                self.visit_expr(&mut repeat_loop.condition);
                self.unhide_from(mark);
            }
            Statement::NumericFor(numeric_for) => {
                self.visit_expr(&mut numeric_for.start);
                self.visit_expr(&mut numeric_for.limit);
                if let Some(step) = &mut numeric_for.step {
                    self.visit_expr(step);
                }
                let mark = self.hidden_log.len();
                self.hide(numeric_for.name.text);
                self.visit_block(&mut numeric_for.block);
                self.unhide_from(mark);
            }
            Statement::GenericFor(generic_for) => {
                for expr in &mut generic_for.exprs {
                    self.visit_expr(expr);
                }
                let mark = self.hidden_log.len();
                for param in &generic_for.names {
                    self.hide(param.name.text);
                }
                self.visit_block(&mut generic_for.block);
                self.unhide_from(mark);
            }
            // `function g.f()` and `function g:f()` read `g` to store into it. A
            // plain `function g()` only assigns it.
            Statement::FunctionDecl(function_declaration) => {
                if let [root, nested @ ..] = &*function_declaration.name.names
                    && (!nested.is_empty() || function_declaration.name.method.is_some())
                {
                    self.read(root.text);
                }
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::IfStatement(_)
            | Statement::LocalFunction(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => self.walk_stmt(stmt),
        }
    }

    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        if let Expression::Name(name) = expr {
            self.read(name.text);
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &mut Var<'ast>) {
        if let Var::Name(name) = target {
            self.read(name.text);
            return;
        }
        self.walk_assignment_target(target);
    }

    // A function body's reads belong to its own scope.
    fn visit_function_body(&mut self, _body: &mut FunctionBody<'ast>) {}

    // Types never run, so they read nothing.
    fn visit_type(&mut self, _ty: &mut Type<'ast>) {}
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::fixtures::{RENAMING, assert_lua55_renames, assert_luau_renames, minify_with};

    fn apply(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &RENAMING)
    }

    fn apply_luau(source: &str) -> String {
        minify_with(source, LuaVersion::Luau, &RENAMING)
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
}
