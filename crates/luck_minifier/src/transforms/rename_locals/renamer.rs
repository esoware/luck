//! The rewrite that gives each binding the name the analyzer picked.

use luck_ast::{
    AttributedName, FunctionBody, FunctionCall, IfStatement, Parameter, Punctuated, Statement,
    Transform, Var,
};
use luck_token::CompactString;
use rustc_hash::FxHashMap;

use crate::binding::{identifier_name, identifier_token};

/// Declares bindings in the order the analyzer did, so binding ids line up,
/// and rewrites every declaration and reference to its binding's new name.
pub(super) struct Renamer {
    binding_new_names: Vec<CompactString>,
    next_binding_id: usize,
    name_stacks: FxHashMap<CompactString, Vec<CompactString>>,
    scope_binding_names: Vec<Vec<CompactString>>,
}

impl Renamer {
    pub(super) fn new(binding_new_names: Vec<CompactString>) -> Self {
        Self {
            binding_new_names,
            next_binding_id: 0,
            name_stacks: FxHashMap::default(),
            scope_binding_names: vec![Vec::new()],
        }
    }

    /// Declares the globals being renamed, as `Analyzer::declare_root_globals`
    /// did.
    pub(super) fn declare_root_globals(&mut self, names: &[&str]) {
        for name in names {
            self.declare_binding(name);
        }
    }

    fn declare_binding(&mut self, original: &str) -> CompactString {
        #[expect(
            clippy::indexing_slicing,
            reason = "out-of-step walks must panic; a fallback would emit wrong Lua"
        )]
        let new_name = self.binding_new_names[self.next_binding_id].clone();
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

    fn transform_if_scoped(&mut self, mut if_stmt: Box<IfStatement>) -> Box<IfStatement> {
        if_stmt.condition = self.transform_expression(if_stmt.condition);
        self.enter_scope();
        if_stmt.block = self.transform_block(if_stmt.block);
        self.exit_scope();
        if_stmt.elseif_clauses = if_stmt
            .elseif_clauses
            .into_iter()
            .map(|mut clause| {
                clause.condition = self.transform_expression(clause.condition);
                self.enter_scope();
                clause.block = self.transform_block(clause.block);
                self.exit_scope();
                clause
            })
            .collect();
        if_stmt.else_clause = if_stmt.else_clause.map(|mut else_clause| {
            self.enter_scope();
            else_clause.block = self.transform_block(else_clause.block);
            self.exit_scope();
            else_clause
        });
        if_stmt
    }

    /// Declares each name a `local` binds, keeping its type annotation and
    /// `<attrib>`.
    fn declare_attributed_names(
        &mut self,
        names: Punctuated<AttributedName>,
    ) -> Punctuated<AttributedName> {
        names.map_items(|mut attributed| {
            let new_name = self.declare_binding(identifier_name(&attributed.name));
            attributed.name = identifier_token(new_name);
            attributed
        })
    }

    fn declare_params(&mut self, params: Punctuated<Parameter>) -> Punctuated<Parameter> {
        params.map_items(|mut param| {
            let new_name = self.declare_binding(identifier_name(&param.name));
            param.name = identifier_token(new_name);
            param
        })
    }
}

impl Transform for Renamer {
    fn transform_statement(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::LocalAssignment(mut local) => {
                // RHS before LHS: `local x = x` reads the outer x.
                local.exprs = local.exprs.map(|exprs| self.walk_punctuated_exprs(exprs));
                local.names = self.declare_attributed_names(local.names);
                Statement::LocalAssignment(local)
            }
            Statement::LocalFunction(mut local_function) => {
                let new_name = self.declare_binding(identifier_name(&local_function.name));
                local_function.name = identifier_token(new_name);
                local_function.body = self.transform_function_body(local_function.body);
                Statement::LocalFunction(local_function)
            }
            Statement::FunctionDecl(mut function_declaration) => {
                function_declaration.body = self.transform_function_body(function_declaration.body);
                if let Some(root_name) = function_declaration.name.names.first_mut() {
                    let resolved = self.resolve_name(identifier_name(root_name));
                    *root_name = identifier_token(resolved);
                }
                Statement::FunctionDecl(function_declaration)
            }
            Statement::DoBlock(mut do_block) => {
                self.enter_scope();
                do_block.block = self.transform_block(do_block.block);
                self.exit_scope();
                Statement::DoBlock(do_block)
            }
            Statement::WhileLoop(mut while_loop) => {
                while_loop.condition = self.transform_expression(while_loop.condition);
                self.enter_scope();
                while_loop.block = self.transform_block(while_loop.block);
                self.exit_scope();
                Statement::WhileLoop(while_loop)
            }
            Statement::RepeatLoop(mut repeat_loop) => {
                self.enter_scope();
                repeat_loop.block = self.transform_block(repeat_loop.block);
                repeat_loop.condition = self.transform_expression(repeat_loop.condition);
                self.exit_scope();
                Statement::RepeatLoop(repeat_loop)
            }
            Statement::IfStatement(if_stmt) => {
                Statement::IfStatement(self.transform_if_scoped(if_stmt))
            }
            Statement::NumericFor(mut numeric_for) => {
                numeric_for.start = self.transform_expression(numeric_for.start);
                numeric_for.limit = self.transform_expression(numeric_for.limit);
                numeric_for.step = numeric_for.step.map(|step| self.transform_expression(step));
                self.enter_scope();
                let new_name = self.declare_binding(identifier_name(&numeric_for.name));
                numeric_for.name = identifier_token(new_name);
                numeric_for.block = self.transform_block(numeric_for.block);
                self.exit_scope();
                Statement::NumericFor(numeric_for)
            }
            Statement::GenericFor(mut generic_for) => {
                generic_for.exprs = self.walk_punctuated_exprs(generic_for.exprs);
                self.enter_scope();
                generic_for.names = self.declare_params(generic_for.names);
                generic_for.block = self.transform_block(generic_for.block);
                self.exit_scope();
                Statement::GenericFor(generic_for)
            }
            // Lua 5.5: declared like a local, so it shadows a same-named local
            // until its scope ends, but it keeps its name.
            Statement::GlobalFunction(mut global_function) => {
                self.declare_binding(identifier_name(&global_function.name));
                global_function.body = self.transform_function_body(global_function.body);
                Statement::GlobalFunction(global_function)
            }
            // Lua 5.5
            Statement::GlobalDeclaration(mut global_declaration) => {
                global_declaration.exprs =
                    global_declaration.exprs.map(|exprs| self.walk_punctuated_exprs(exprs));
                for attributed in global_declaration.names.iter() {
                    self.declare_binding(identifier_name(&attributed.name));
                }
                Statement::GlobalDeclaration(global_declaration)
            }
            // `walk_statement` visits these in the same order as
            // `Analyzer::analyze_stmt`, which keeps the binding ids in step.
            other @ (Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::CompoundAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_)) => self.walk_statement(other),
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
        self.enter_scope();
        body.params = self.declare_params(body.params);
        body.block = self.transform_block(body.block);
        self.exit_scope();
        body
    }

    fn transform_function_call(&mut self, mut call: FunctionCall) -> FunctionCall {
        call.callee = self.transform_expression(call.callee);
        call.explicit_type_args = call.explicit_type_args.map(|mut type_args| {
            *type_args = self.walk_type_args(*type_args);
            type_args
        });
        call.args = self.transform_function_args(call.args);
        call
    }
}
