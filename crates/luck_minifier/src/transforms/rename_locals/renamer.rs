use core::mem;

use luck_ast::{
    AttributedName, Block, FunctionBody, FunctionDecl, GenericFor, GlobalDeclaration,
    GlobalFunction, IfStatement, LocalAssignment, LocalFunction, NumericFor, Parameter, Punctuated,
    RepeatLoop, Statement, Transform, Var,
};
use luck_token::{CompactString, LuaVersion, Span};
use rustc_hash::FxHashMap;

use crate::binding::{
    IMPLICIT_VARARG_TABLE_NAME, METHOD_RECEIVER_NAME, has_implicit_arg, identifier_name,
    identifier_token,
};

/// A binding as the analyzer declared it, and the name it gets.
pub(super) struct RenamedBinding {
    original_name: CompactString,
    new_name: CompactString,
}

impl RenamedBinding {
    pub(super) const fn new(original_name: CompactString, new_name: CompactString) -> Self {
        Self { original_name, new_name }
    }
}

/// Declares bindings in the order the analyzer did, so binding ids line up,
/// and rewrites every declaration and reference to its binding's new name.
pub(super) struct Renamer {
    bindings: Vec<RenamedBinding>,
    next_binding_id: usize,
    /// Whether a declaration didn't match the analyzer's binding at its id.
    /// The names handed out from there on belong to other bindings, so the
    /// tree is wrong.
    is_out_of_step: bool,
    name_stacks: FxHashMap<CompactString, Vec<CompactString>>,
    scope_binding_names: Vec<Vec<CompactString>>,
    version: LuaVersion,
}

impl Renamer {
    pub(super) fn new(bindings: Vec<RenamedBinding>, version: LuaVersion) -> Self {
        Self {
            bindings,
            next_binding_id: 0,
            is_out_of_step: false,
            name_stacks: FxHashMap::default(),
            scope_binding_names: vec![Vec::new()],
            version,
        }
    }

    /// Whether the walk declared exactly the analyzer's bindings, in order.
    pub(super) const fn is_in_step(&self) -> bool {
        !self.is_out_of_step && self.next_binding_id == self.bindings.len()
    }

    pub(super) fn declare_root_globals(&mut self, names: &[&str]) {
        for name in names {
            self.declare_binding(name);
        }
    }

    fn declare_binding(&mut self, original: &str) -> CompactString {
        let new_name = match self.bindings.get(self.next_binding_id) {
            Some(binding) if binding.original_name == original => binding.new_name.clone(),
            Some(_) | None => {
                self.is_out_of_step = true;
                original.into()
            }
        };
        self.next_binding_id = self.next_binding_id.saturating_add(1);
        let original_name = CompactString::from(original);
        if let Some(scope_names) = self.scope_binding_names.last_mut() {
            scope_names.push(original_name.clone());
        }
        self.name_stacks.entry(original_name).or_default().push(new_name.clone());
        new_name
    }

    fn enter_scope(&mut self) {
        self.scope_binding_names.push(Vec::new());
    }

    fn exit_scope(&mut self) {
        if let Some(scope_names) = self.scope_binding_names.pop() {
            for original in scope_names.iter().rev() {
                if let Some(stack) = self.name_stacks.get_mut(original) {
                    stack.pop();
                    if stack.is_empty() {
                        self.name_stacks.remove(original);
                    }
                }
            }
        }
    }

    fn resolve_name(&self, original: &str) -> CompactString {
        if let Some(stack) = self.name_stacks.get(original)
            && let Some(new_name) = stack.last()
        {
            return new_name.clone();
        }
        original.into()
    }

    /// Transforms a block in a scope of its own, in place.
    fn transform_scoped_block(&mut self, block: &mut Block) {
        let empty = Block { span: Span::default(), stmts: Vec::new(), last_stmt: None };
        self.enter_scope();
        *block = self.transform_block(mem::replace(block, empty));
        self.exit_scope();
    }

    fn transform_if_scoped(&mut self, mut if_stmt: Box<IfStatement>) -> Box<IfStatement> {
        if_stmt.condition = self.transform_expr(if_stmt.condition);
        self.transform_scoped_block(&mut if_stmt.block);
        for clause in &mut if_stmt.elseif_clauses {
            clause.condition = self.transform_expr(clause.condition.take());
            self.transform_scoped_block(&mut clause.block);
        }
        if let Some(else_clause) = &mut if_stmt.else_clause {
            self.transform_scoped_block(&mut else_clause.block);
        }
        if_stmt
    }

    // Each statement below is renamed in a function of its own, so a level of
    // nesting holds only that statement's temporaries on the stack.
    fn rename_local(&mut self, mut local: Box<LocalAssignment>) -> Statement {
        // RHS before LHS: `local x = x` reads the outer x.
        local.exprs = local.exprs.map(|exprs| self.walk_punctuated_exprs(exprs));
        local.names = self.declare_attributed_names(local.names);
        Statement::LocalAssignment(local)
    }

    fn rename_local_function(&mut self, mut local_function: Box<LocalFunction>) -> Statement {
        self.transform_signature_annotations(&mut local_function.body);
        let new_name = self.declare_binding(identifier_name(&local_function.name));
        local_function.name = identifier_token(new_name);
        local_function.body = self.transform_function_scope(local_function.body, false);
        Statement::LocalFunction(local_function)
    }

    fn rename_function_decl(&mut self, mut function_declaration: Box<FunctionDecl>) -> Statement {
        self.transform_signature_annotations(&mut function_declaration.body);
        function_declaration.body = self.transform_function_scope(
            function_declaration.body,
            function_declaration.name.method.is_some(),
        );
        if let Some(root_name) = function_declaration.name.names.first_mut() {
            let resolved = self.resolve_name(identifier_name(root_name));
            *root_name = identifier_token(resolved);
        }
        Statement::FunctionDecl(function_declaration)
    }

    fn rename_repeat_loop(&mut self, mut repeat_loop: Box<RepeatLoop>) -> Statement {
        self.enter_scope();
        repeat_loop.block = self.transform_block(repeat_loop.block);
        repeat_loop.condition = self.transform_expr(repeat_loop.condition);
        self.exit_scope();
        Statement::RepeatLoop(repeat_loop)
    }

    fn rename_numeric_for(&mut self, mut numeric_for: Box<NumericFor>) -> Statement {
        numeric_for.start = self.transform_expr(numeric_for.start);
        numeric_for.limit = self.transform_expr(numeric_for.limit);
        numeric_for.step = numeric_for.step.map(|step| self.transform_expr(step));
        // Luau
        numeric_for.type_annotation = numeric_for.type_annotation.map(|ty| self.transform_type(ty));
        self.enter_scope();
        let new_name = self.declare_binding(identifier_name(&numeric_for.name));
        numeric_for.name = identifier_token(new_name);
        numeric_for.block = self.transform_block(numeric_for.block);
        self.exit_scope();
        Statement::NumericFor(numeric_for)
    }

    fn rename_generic_for(&mut self, mut generic_for: Box<GenericFor>) -> Statement {
        generic_for.exprs = self.walk_punctuated_exprs(generic_for.exprs);
        self.transform_param_annotations(&mut generic_for.names);
        self.enter_scope();
        generic_for.names = self.declare_params(generic_for.names);
        generic_for.block = self.transform_block(generic_for.block);
        self.exit_scope();
        Statement::GenericFor(generic_for)
    }

    /// Lua 5.5: declared like a local, so it shadows a same-named local until
    /// its scope ends, but it keeps its name.
    fn rename_global_function(&mut self, mut global_function: Box<GlobalFunction>) -> Statement {
        self.declare_binding(identifier_name(&global_function.name));
        global_function.body = self.transform_function_body(global_function.body);
        Statement::GlobalFunction(global_function)
    }

    // Lua 5.5
    fn rename_global_declaration(
        &mut self,
        mut global_declaration: Box<GlobalDeclaration>,
    ) -> Statement {
        global_declaration.exprs =
            global_declaration.exprs.map(|exprs| self.walk_punctuated_exprs(exprs));
        for attributed in global_declaration.names.iter() {
            self.declare_binding(identifier_name(&attributed.name));
        }
        Statement::GlobalDeclaration(global_declaration)
    }

    fn declare_attributed_names(
        &mut self,
        mut names: Punctuated<AttributedName>,
    ) -> Punctuated<AttributedName> {
        for attributed in &mut names.items {
            attributed.type_annotation =
                attributed.type_annotation.take().map(|ty| self.transform_type(ty));
        }
        names.map_items(|mut attributed| {
            let new_name = self.declare_binding(identifier_name(&attributed.name));
            attributed.name = identifier_token(new_name);
            attributed
        })
    }

    fn transform_param_annotations(&mut self, params: &mut Punctuated<Parameter>) {
        for param in &mut params.items {
            param.type_annotation = param.type_annotation.take().map(|ty| self.transform_type(ty));
        }
    }

    fn declare_params(&mut self, params: Punctuated<Parameter>) -> Punctuated<Parameter> {
        params.map_items(|mut param| {
            let new_name = self.declare_binding(identifier_name(&param.name));
            param.name = identifier_token(new_name);
            param
        })
    }

    /// Luau: a function's signature annotations, in the order the analyzer
    /// reached them, before anything the function binds.
    fn transform_signature_annotations(&mut self, body: &mut FunctionBody) {
        body.generics = body.generics.take().map(|mut generics| {
            *generics = self.walk_generic_type_list(*generics);
            generics
        });
        self.transform_param_annotations(&mut body.params);
        if let Some(vararg) = body.vararg.as_mut() {
            vararg.type_annotation =
                vararg.type_annotation.take().map(|ty| self.transform_type(ty));
        }
        body.return_type = body.return_type.take().map(|ty| self.transform_type(ty));
    }

    /// Scopes a function's parameters and implicit names, in the order the
    /// analyzer declared them, and transforms its block.
    fn transform_function_scope(
        &mut self,
        mut body: FunctionBody,
        is_method: bool,
    ) -> FunctionBody {
        self.enter_scope();
        if is_method {
            self.declare_binding(METHOD_RECEIVER_NAME);
        }
        body.params = self.declare_params(body.params);
        if has_implicit_arg(&body, self.version) {
            self.declare_binding(IMPLICIT_VARARG_TABLE_NAME);
        }
        // Lua 5.5
        if let Some(vararg_name) = body.vararg.as_mut().and_then(|vararg| vararg.name.as_mut()) {
            let new_name = self.declare_binding(identifier_name(vararg_name));
            *vararg_name = identifier_token(new_name);
        }
        body.block = self.transform_block(body.block);
        self.exit_scope();
        body
    }
}

impl Transform for Renamer {
    fn transform_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::LocalAssignment(local) => self.rename_local(local),
            Statement::LocalFunction(local_function) => self.rename_local_function(local_function),
            Statement::FunctionDecl(function_declaration) => {
                self.rename_function_decl(function_declaration)
            }
            Statement::DoBlock(mut do_block) => {
                self.transform_scoped_block(&mut do_block.block);
                Statement::DoBlock(do_block)
            }
            Statement::WhileLoop(mut while_loop) => {
                while_loop.condition = self.transform_expr(while_loop.condition);
                self.transform_scoped_block(&mut while_loop.block);
                Statement::WhileLoop(while_loop)
            }
            Statement::RepeatLoop(repeat_loop) => self.rename_repeat_loop(repeat_loop),
            Statement::IfStatement(if_stmt) => {
                Statement::IfStatement(self.transform_if_scoped(if_stmt))
            }
            Statement::NumericFor(numeric_for) => self.rename_numeric_for(numeric_for),
            Statement::GenericFor(generic_for) => self.rename_generic_for(generic_for),
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.rename_global_function(global_function)
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global_declaration) => {
                self.rename_global_declaration(global_declaration)
            }
            // `walk_stmt` visits these in the same order as the
            // analyzer's `Visitor` walk, which keeps the binding ids in step.
            other @ (Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::CompoundAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_)) => self.walk_stmt(other),
        }
    }

    fn transform_var(&mut self, var: Var) -> Var {
        match var {
            Var::Name(name) => {
                let resolved = self.resolve_name(identifier_name(&name));
                Var::Name(identifier_token(resolved))
            }
            other @ (Var::Index(_) | Var::FieldAccess(_)) => self.walk_var(other),
        }
    }

    fn transform_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        self.transform_signature_annotations(&mut body);
        self.transform_function_scope(body, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::parse;

    fn binding(original_name: &str, new_name: &str) -> RenamedBinding {
        RenamedBinding::new(original_name.into(), new_name.into())
    }

    fn is_in_step_after(source: &str, bindings: Vec<RenamedBinding>) -> bool {
        let mut renamer = Renamer::new(bindings, LuaVersion::Lua54);
        let _renamed = renamer.transform_block(parse(source, LuaVersion::Lua54));
        renamer.is_in_step()
    }

    #[test]
    fn a_walk_that_declares_the_analyzers_bindings_is_in_step() {
        let bindings = vec![binding("first", "l"), binding("second", "u")];
        assert!(is_in_step_after("local first = 1 local second = first", bindings));
    }

    #[test]
    fn a_walk_past_the_analyzers_bindings_is_out_of_step() {
        assert!(!is_in_step_after("local first = 1", Vec::new()));
        assert!(!is_in_step_after("local first, second = 1, 2", vec![binding("first", "l")]));
    }

    #[test]
    fn a_walk_short_of_the_analyzers_bindings_is_out_of_step() {
        let bindings = vec![binding("first", "l"), binding("second", "u")];
        assert!(!is_in_step_after("local first = 1", bindings));
    }

    #[test]
    fn a_walk_that_declares_another_name_is_out_of_step() {
        let bindings = vec![binding("second", "l"), binding("first", "u")];
        assert!(!is_in_step_after("local first = 1 local second = first", bindings));
    }
}
