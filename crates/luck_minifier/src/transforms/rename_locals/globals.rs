use luck_ast::{Assignment, Block, Expression, FunctionBody, Statement, Type, Var, Visitor};
use luck_token::{CompactString, LuaVersion, Token};
use rustc_hash::FxHashSet;

use crate::binding::{identifier_name, is_env_binding};

/// The globals to rename, in the order the analyzer and the renamer declare
/// them as root bindings: each top-level `function g()` name, then each name
/// a top-level assignment defines.
pub(super) fn renamable_globals(block: &Block, version: LuaVersion) -> Vec<&str> {
    let mut declared_globals = DeclaredGlobals::default();
    declared_globals.visit_block(block);
    let function_globals = collect_toplevel_function_globals(block, &declared_globals.0, version);
    let assign_first_globals =
        collect_assign_first_globals(block, &function_globals, &declared_globals.0, version);

    let mut names = Vec::new();
    let mut declared_names = FxHashSet::default();
    for stmt in &block.stmts {
        if let Statement::FunctionDecl(function_declaration) = stmt
            && let [name_token] = function_declaration.name.names.as_slice()
            && function_declaration.name.method.is_none()
        {
            let name = identifier_name(name_token);
            if function_globals.contains(name) {
                names.push(name);
                declared_names.insert(name);
            }
        }
    }
    for stmt in &block.stmts {
        if let Statement::Assignment(assignment) = stmt {
            for var in assignment.targets.iter() {
                if let Var::Name(name_token) = var {
                    let name = identifier_name(name_token);
                    if assign_first_globals.contains(name) && declared_names.insert(name) {
                        names.push(name);
                    }
                }
            }
        }
    }
    names
}

/// A name declared anywhere is the same variable everywhere, so one
/// declaration keeps the whole file from renaming it.
#[derive(Default)]
struct DeclaredGlobals(FxHashSet<CompactString>);

impl<'ast> Visitor<'ast> for DeclaredGlobals {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            // Lua 5.5
            Statement::GlobalDeclaration(global_declaration) => {
                for attributed in global_declaration.names.iter() {
                    self.0.insert(identifier_name(&attributed.name).into());
                }
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.0.insert(identifier_name(&global_function.name).into());
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

fn collect_toplevel_function_globals(
    block: &Block,
    declared_globals: &FxHashSet<CompactString>,
    version: LuaVersion,
) -> FxHashSet<CompactString> {
    let mut top_locals: FxHashSet<CompactString> = FxHashSet::default();
    for stmt in &block.stmts {
        match stmt {
            Statement::LocalAssignment(local) => {
                for attributed in local.names.iter() {
                    top_locals.insert(identifier_name(&attributed.name).into());
                }
            }
            Statement::LocalFunction(local_function) => {
                top_locals.insert(identifier_name(&local_function.name).into());
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
    let mut assigned_first_names: FxHashSet<CompactString> = FxHashSet::default();
    let mut function_globals = FxHashSet::default();
    for stmt in &block.stmts {
        if let Statement::Assignment(assignment) = stmt {
            reads.visit_assignment_reads(assignment);
            for var in assignment.targets.iter() {
                if let Var::Name(name) = var
                    && !reads.names.contains(identifier_name(name))
                {
                    assigned_first_names.insert(identifier_name(name).into());
                }
            }
        } else if let Statement::FunctionDecl(function_declaration) = stmt
            && let [name_token] = function_declaration.name.names.as_slice()
            && function_declaration.name.method.is_none()
        {
            let name = identifier_name(name_token);
            let is_read_first = reads.names.contains(name) && !assigned_first_names.contains(name);
            if !top_locals.contains(name)
                && !is_read_first
                && !declared_globals.contains(name)
                && !is_env_binding(name, version)
            {
                function_globals.insert(name.into());
            }
        } else {
            reads.visit_stmt(stmt);
        }
    }

    function_globals
}

fn collect_assign_first_globals(
    block: &Block,
    function_globals: &FxHashSet<CompactString>,
    declared_globals: &FxHashSet<CompactString>,
    version: LuaVersion,
) -> FxHashSet<CompactString> {
    let mut chunk_locals = ChunkLocals::default();
    chunk_locals.visit_block(block);
    let all_locals = chunk_locals.0;

    let mut reads = NameReads::new(&all_locals);
    let mut assign_first_globals: FxHashSet<CompactString> = FxHashSet::default();
    for stmt in &block.stmts {
        match stmt {
            Statement::Assignment(assignment) => {
                reads.visit_assignment_reads(assignment);
                for var in assignment.targets.iter() {
                    if let Var::Name(name) = var {
                        let var_name = identifier_name(name);
                        if !all_locals.contains(var_name)
                            && !function_globals.contains(var_name)
                            && !reads.names.contains(var_name)
                            && !assign_first_globals.contains(var_name)
                            && !declared_globals.contains(var_name)
                            && !is_env_binding(var_name, version)
                        {
                            assign_first_globals.insert(var_name.into());
                        }
                    }
                }
            }
            // Everything else only contributes reads.
            Statement::FunctionCall(_)
            | Statement::LocalAssignment(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::GlobalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => reads.visit_stmt(stmt),
        }
    }

    assign_first_globals
}

#[derive(Default)]
struct ChunkLocals(FxHashSet<CompactString>);

impl<'ast> Visitor<'ast> for ChunkLocals {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                for attributed in local.names.iter() {
                    self.0.insert(identifier_name(&attributed.name).into());
                }
            }
            Statement::LocalFunction(local_function) => {
                self.0.insert(identifier_name(&local_function.name).into());
            }
            Statement::NumericFor(numeric_for) => {
                self.0.insert(identifier_name(&numeric_for.name).into());
            }
            Statement::GenericFor(generic_for) => {
                for param in generic_for.names.iter() {
                    self.0.insert(identifier_name(&param.name).into());
                }
            }
            // Nothing else binds a local. `walk_stmt` below still
            // descends into nested blocks.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::FunctionDecl(_)
            | Statement::GlobalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
    }

    // Only a function body nests a block inside an expression.
    fn visit_expr(&mut self, _expr: &'ast Expression) {}

    fn visit_function_body(&mut self, _body: &'ast FunctionBody) {}
}

/// The names statements read as globals: each name outside `locals` and
/// outside the locals the enclosing nested blocks have declared so far.
struct NameReads<'locals> {
    locals: &'locals FxHashSet<CompactString>,
    hidden_names: FxHashSet<CompactString>,
    /// What `hidden_names` gained, in order, so each scope can drop its own on exit.
    hidden_log: Vec<CompactString>,
    names: FxHashSet<CompactString>,
}

impl<'locals> NameReads<'locals> {
    fn new(locals: &'locals FxHashSet<CompactString>) -> Self {
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

    fn read(&mut self, name_token: &Token) {
        let name = identifier_name(name_token);
        if !self.is_local(name) {
            self.names.insert(name.into());
        }
    }

    fn hide(&mut self, name: &str) {
        if !self.is_local(name) {
            self.hidden_names.insert(name.into());
            self.hidden_log.push(name.into());
        }
    }

    fn unhide_from(&mut self, mark: usize) {
        for name in self.hidden_log.drain(mark..) {
            self.hidden_names.remove(&name);
        }
    }

    /// A block's own locals don't count as reads. Slot reuse gives nested
    /// locals the names of renamed globals, so on every later round they'd
    /// otherwise look like reads of those globals.
    fn visit_scope(&mut self, block: &Block) -> usize {
        let mark = self.hidden_log.len();
        for stmt in &block.stmts {
            self.visit_stmt(stmt);
            match stmt {
                Statement::LocalAssignment(local) => {
                    for attributed in local.names.iter() {
                        self.hide(identifier_name(&attributed.name));
                    }
                }
                Statement::LocalFunction(local_function) => {
                    self.hide(identifier_name(&local_function.name));
                }
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
        if let Some(last) = &block.last_stmt {
            self.visit_last_stmt(last);
        }
        mark
    }

    /// Every target's prefix and index is evaluated before any store, so
    /// `g, t[g] = 1, 2` reads `g` even though its own target comes first.
    fn visit_assignment_reads(&mut self, assignment: &Assignment) {
        for expr in assignment.values.iter() {
            self.visit_expr(expr);
        }
        for var in assignment.targets.iter() {
            if !matches!(var, Var::Name(_)) {
                self.visit_assignment_target(var);
            }
        }
    }
}

impl<'ast> Visitor<'ast> for NameReads<'_> {
    fn visit_block(&mut self, block: &'ast Block) {
        let mark = self.visit_scope(block);
        self.unhide_from(mark);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::RepeatLoop(repeat_loop) => {
                // The condition sees the block's locals.
                let mark = self.visit_scope(&repeat_loop.block);
                self.visit_expr(&repeat_loop.condition);
                self.unhide_from(mark);
            }
            Statement::NumericFor(numeric_for) => {
                self.visit_expr(&numeric_for.start);
                self.visit_expr(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.visit_expr(step);
                }
                let mark = self.hidden_log.len();
                self.hide(identifier_name(&numeric_for.name));
                self.visit_block(&numeric_for.block);
                self.unhide_from(mark);
            }
            Statement::GenericFor(generic_for) => {
                for expr in generic_for.exprs.iter() {
                    self.visit_expr(expr);
                }
                let mark = self.hidden_log.len();
                for param in generic_for.names.iter() {
                    self.hide(identifier_name(&param.name));
                }
                self.visit_block(&generic_for.block);
                self.unhide_from(mark);
            }
            // `function g.f()` and `function g:f()` read `g` to store into it. A
            // plain `function g()` only assigns it.
            Statement::FunctionDecl(function_declaration) => {
                if let [root_name, nested_names @ ..] = function_declaration.name.names.as_slice()
                    && (!nested_names.is_empty() || function_declaration.name.method.is_some())
                {
                    self.read(root_name);
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

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Name(name) = expr {
            self.read(name);
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &'ast Var) {
        if let Var::Name(name) = target {
            self.read(name);
        }
        self.walk_assignment_target(target);
    }

    // A function body's reads belong to its own scope.
    fn visit_function_body(&mut self, _body: &'ast FunctionBody) {}

    // Types never run, so they read nothing.
    fn visit_type(&mut self, _ty: &'ast Type) {}
}
