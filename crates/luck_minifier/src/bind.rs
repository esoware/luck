//! The bind walk, which runs once, before anything else rewrites the tree.
//!
//! It resolves every name to its binding with the target's scoping rules,
//! writes each binding's identity into its names, counts each binding's
//! references, and measures how deep the tree nests.

use core::mem;

use luck_ast::{
    Block, Expression, FunctionBody, LocalFunction, MAX_NESTING_DEPTH, Name, Statement, Type, Var,
    VisitMut,
};
use luck_token::LuaVersion;

use crate::binding::{
    Bindings, IMPLICIT_VARARG_TABLE_NAME, METHOD_RECEIVER_NAME, NONE, Reference, global_index,
    has_implicit_arg, is_env_binding, local_index,
};
use crate::name_table::NameTable;

/// Binds `block`, whose names `names` numbers, into `bindings`, and says
/// whether it nests within [`MAX_NESTING_DEPTH`].
pub(crate) fn bind<'ast>(
    block: &mut Block<'ast>,
    version: LuaVersion,
    bindings: &mut Bindings,
    names: &mut NameTable<'ast>,
) -> bool {
    bindings.clear();
    names.clear();
    names.intern(METHOD_RECEIVER_NAME);
    names.intern(IMPLICIT_VARARG_TABLE_NAME);
    let mut binder = Binder {
        bindings,
        names,
        version,
        innermost: Vec::new(),
        declarations: Vec::new(),
        scope_starts: Vec::new(),
        function_depth: 0,
        type_floor: 0,
        depth: 0,
        deepest: 0,
    };
    binder.visit_block(block);
    let name_count = binder.names.len();
    binder.bindings.resize_names(name_count);
    usize::try_from(binder.deepest).is_ok_and(|deepest| deepest <= MAX_NESTING_DEPTH)
}

/// A declaration in an open scope.
struct Declaration {
    name: usize,
    /// The declaration of the same name it hides, or [`NONE`].
    hidden: u32,
    /// Its binding, or [`NONE`] for a Lua 5.5 `global` declaration.
    binding: u32,
}

struct Binder<'walk, 'ast> {
    bindings: &'walk mut Bindings,
    names: &'walk mut NameTable<'ast>,
    version: LuaVersion,
    /// Each name's innermost open declaration, by name id.
    innermost: Vec<u32>,
    declarations: Vec<Declaration>,
    scope_starts: Vec<usize>,
    function_depth: u32,
    /// One past the function depth of the innermost type the walk is in.
    type_floor: u32,
    /// Levels of nesting open where the walk is, and the most there have
    /// been.
    depth: u32,
    deepest: u32,
}

impl<'ast> Binder<'_, 'ast> {
    fn name_id(&mut self, text: &'ast str) -> usize {
        let id = self.names.intern(text);
        if self.innermost.len() <= id {
            self.innermost.resize(id.saturating_add(1), NONE);
            self.bindings.resize_names(id.saturating_add(1));
        }
        id
    }

    fn enter_scope(&mut self) {
        self.scope_starts.push(self.declarations.len());
    }

    fn exit_scope(&mut self) {
        let start = self.scope_starts.pop().unwrap_or(0);
        for declaration in self.declarations.drain(start..).rev() {
            if let Some(innermost) = self.innermost.get_mut(declaration.name) {
                *innermost = declaration.hidden;
            }
        }
    }

    fn open(&mut self, name: usize, binding: u32) {
        let Some(innermost) = self.innermost.get_mut(name) else {
            return;
        };
        let hidden = *innermost;
        *innermost = u32::try_from(self.declarations.len()).unwrap_or(NONE);
        self.declarations.push(Declaration { name, hidden, binding });
    }

    /// Declares a binding with no name of its own in the tree.
    fn declare_implicit(&mut self, text: &'ast str) {
        let name = self.name_id(text);
        let id = self.bindings.push(name, true, self.function_depth);
        self.open(name, u32::try_from(id).unwrap_or(NONE));
    }

    fn declare(&mut self, name: &mut Name<'ast>, is_exported: bool) -> usize {
        let name_id = self.name_id(name.text.as_str());
        let is_fixed = is_exported || is_env_binding(name.text.as_str(), self.version);
        let id = self.bindings.push(name_id, is_fixed, self.function_depth);
        name.index = Some(local_index(id));
        self.open(name_id, u32::try_from(id).unwrap_or(NONE));
        id
    }

    /// Lua 5.5: a `global` declaration scopes and shadows like a local, and
    /// its references still reach the global.
    fn declare_global(&mut self, name: &mut Name<'ast>) {
        let name_id = self.name_id(name.text.as_str());
        name.index = Some(global_index(name_id));
        self.bindings.count_global_declaration(name, false);
        self.open(name_id, NONE);
    }

    fn reference(&mut self, name: &mut Name<'ast>, is_write: bool) {
        let name_id = self.name_id(name.text.as_str());
        let reference = Reference {
            is_write,
            function_depth: self.function_depth,
            type_floor: self.type_floor,
            depth: self.depth,
        };
        let binding = self
            .innermost
            .get(name_id)
            .and_then(|&innermost| self.declarations.get(usize::try_from(innermost).ok()?))
            .map_or(NONE, |declaration| declaration.binding);
        if binding == NONE {
            name.index = Some(global_index(name_id));
            self.bindings.count_global(name_id, reference, false);
        } else {
            let id = usize::try_from(binding).unwrap_or(usize::MAX);
            name.index = Some(local_index(id));
            self.bindings.count(id, reference, false);
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

    /// A function's signature, then the name of a `local function`, which
    /// its body sees, then its parameters and implicit names, and then its
    /// block, the whole one level deeper as the walks count it.
    fn visit_function(
        &mut self,
        body: &mut FunctionBody<'ast>,
        is_method: bool,
        local_function: Option<(&mut Name<'ast>, bool)>,
    ) {
        if self.descend() {
            self.visit_signature(body);
            let own_name =
                local_function.map(|(name, is_exported)| self.declare(name, is_exported));
            self.set_open(own_name, true);
            self.function_depth = self.function_depth.saturating_add(1);
            self.enter_scope();
            if is_method {
                self.declare_implicit(METHOD_RECEIVER_NAME);
            }
            for param in &mut body.params {
                self.declare(&mut param.name, false);
            }
            if has_implicit_arg(body, self.version) {
                self.declare_implicit(IMPLICIT_VARARG_TABLE_NAME);
            }
            // Lua 5.5
            if let Some(name) = body.vararg.as_mut().and_then(|vararg| vararg.name.as_mut()) {
                self.declare(name, false);
            }
            self.visit_block(&mut body.block);
            self.exit_scope();
            self.function_depth = self.function_depth.saturating_sub(1);
            self.set_open(own_name, false);
        }
        self.ascend();
    }

    /// A `local function`'s references to itself in its own body keep
    /// nothing alive.
    fn set_open(&mut self, id: Option<usize>, is_open: bool) {
        if let Some(binding) = id.and_then(|own| self.bindings.entries.get_mut(own)) {
            binding.is_open = is_open;
        }
    }

    fn visit_scoped_block(&mut self, block: &mut Block<'ast>) {
        self.enter_scope();
        self.visit_block(block);
        self.exit_scope();
    }
}

impl<'ast> VisitMut<'ast> for Binder<'_, 'ast> {
    /// The walk goes no deeper than one level past the cap, so it's safe on a
    /// tree of any depth.
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        self.deepest = self.deepest.max(self.depth);
        usize::try_from(self.depth).is_ok_and(|depth| depth <= MAX_NESTING_DEPTH)
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

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
                    self.visit_attributed_name(attributed);
                }
                let is_exported = local.kind.is_exported();
                for attributed in &mut local.names {
                    self.declare(&mut attributed.name, is_exported);
                }
            }
            Statement::LocalFunction(local_function) => {
                for attribute in &mut local_function.attributes {
                    self.visit_function_attribute(attribute);
                }
                let is_exported = local_function.kind.is_exported();
                let LocalFunction { name, body, .. } = &mut **local_function;
                self.visit_function(body, false, Some((name, is_exported)));
            }
            Statement::FunctionDecl(function_decl) => {
                for attribute in &mut function_decl.attributes {
                    self.visit_function_attribute(attribute);
                }
                let is_method = function_decl.name.method.is_some();
                self.visit_function(&mut function_decl.body, is_method, None);
                if let Some(root) = function_decl.name.names.first_mut() {
                    self.reference(root, true);
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
                // Luau
                if let Some(ty) = &mut numeric_for.type_annotation {
                    self.visit_type(ty);
                }
                self.visit_expr(&mut numeric_for.start);
                self.visit_expr(&mut numeric_for.limit);
                if let Some(step) = &mut numeric_for.step {
                    self.visit_expr(step);
                }
                self.enter_scope();
                self.declare(&mut numeric_for.name, false);
                self.visit_block(&mut numeric_for.block);
                self.exit_scope();
            }
            Statement::GenericFor(generic_for) => {
                // Luau: every name binds after the whole list parses.
                for param in &mut generic_for.names {
                    if let Some(ty) = &mut param.type_annotation {
                        self.visit_type(ty);
                    }
                }
                for expr in &mut generic_for.exprs {
                    self.visit_expr(expr);
                }
                self.enter_scope();
                for param in &mut generic_for.names {
                    self.declare(&mut param.name, false);
                }
                self.visit_block(&mut generic_for.block);
                self.exit_scope();
            }
            // Lua 5.5: as with `local function`, the body sees the name.
            Statement::GlobalFunction(global_function) => {
                self.declare_global(&mut global_function.name);
                self.visit_function_body(&mut global_function.body);
            }
            // Lua 5.5: as with `local`, the initializers see the outer scope.
            Statement::GlobalDeclaration(global_declaration) => {
                for expr in &mut global_declaration.exprs {
                    self.visit_expr(expr);
                }
                for attributed in &mut global_declaration.names {
                    self.visit_attributed_name(attributed);
                }
                for attributed in &mut global_declaration.names {
                    self.declare_global(&mut attributed.name);
                }
            }
            // These bind nothing themselves. The walk still reaches the
            // functions they hold, Luau `type function` bodies and `typeof`
            // included.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Return(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Error(_) => self.walk_stmt(stmt),
        }
    }

    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        if let Expression::Name(name) = expr {
            self.reference(name, false);
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_var(&mut self, var: &mut Var<'ast>) {
        if let Var::Name(name) = var {
            self.reference(name, true);
            return;
        }
        self.walk_var(var);
    }

    fn visit_function_body(&mut self, function_body: &mut FunctionBody<'ast>) {
        self.visit_function(function_body, false, None);
    }

    fn visit_type(&mut self, ty: &mut Type<'ast>) {
        let outer_floor = mem::replace(&mut self.type_floor, self.function_depth.saturating_add(1));
        self.walk_type(ty);
        self.type_floor = outer_floor;
    }
}
