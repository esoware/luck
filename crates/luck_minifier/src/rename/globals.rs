use luck_ast::{Assignment, Block, Expression, FunctionBody, Statement, Type, Var, VisitorMut};
use luck_token::LuaVersion;
use rustc_hash::FxHashSet;

use crate::binding::is_env_binding;

/// The globals to rename, in the order the renamer declares them as root
/// bindings: each top-level `function g()` name, then each name a top-level
/// assignment defines.
pub(super) fn renamable_globals<'ast>(
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
