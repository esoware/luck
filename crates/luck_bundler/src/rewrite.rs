use luck_allocator::{Allocator, Box, List};
use luck_ast::{
    Block, Expression, FunctionArgs, FunctionName, GetSpan as _, LocalAssignment,
    LocalAssignmentKind, LocalFunction, LocalFunctionKind, MAX_NESTING_DEPTH, Name, OneOrMore,
    Statement, Suffix, SuffixedExpression, Var, VisitMut, spans_past_nesting_cap,
};
use luck_diagnostics::{Diagnostic, FileDiagnostics};
use luck_token::Span;

use crate::module::ModuleInfo;
use crate::plan::{Plan, WRAPPER_LEVELS};

/// The names the rewrites give the loader and the export table, in the
/// arena.
pub(crate) struct LoaderNames<'ast> {
    require: &'ast str,
    exports: &'ast str,
}

impl<'ast> LoaderNames<'ast> {
    pub(crate) fn new(allocator: &'ast Allocator, prefix: &str) -> Self {
        Self {
            require: allocator.alloc_concat_strs_array([prefix, "require"]),
            exports: allocator.alloc_concat_strs_array([prefix, "exports"]),
        }
    }
}

/// Rewrites `block`, `module`'s, in place to what the bundle holds: each
/// require retargeted at the loader, and each export lowered as the module's
/// exports plan. New nodes go in `allocator`.
pub(crate) fn rewrite_module<'ast>(
    allocator: &'ast Allocator,
    block: &mut Block<'ast>,
    module: &ModuleInfo<'ast>,
    plan: &Plan,
    names: &LoaderNames<'ast>,
) {
    let mut sites: Vec<u32> = module
        .dependencies
        .iter()
        .map(|dependency| dependency.call_span.start)
        .chain(module.exports.field_references.iter().map(|reference| reference.start))
        .collect();
    sites.sort_unstable();
    let mut rewriter = ModuleRewriter { allocator, module, plan, names, sites };
    rewriter.visit_block(block);
    for stmt in &mut block.stmts {
        *stmt = rewriter.lower_export(stmt.take());
    }
}

/// An E013 for each construct `block`, `module`'s rewritten tree, nests past
/// [`MAX_NESTING_DEPTH`] in its wrapper, or `None` when none does.
pub(crate) fn overflow_errors(
    block: &Block<'_>,
    module: &ModuleInfo<'_>,
) -> Option<FileDiagnostics> {
    let diags: Vec<Diagnostic> = spans_past_nesting_cap(block, WRAPPER_LEVELS)
        .into_iter()
        .map(|span| Diagnostic::e013(span, WRAPPER_LEVELS, MAX_NESTING_DEPTH))
        .collect();
    (!diags.is_empty()).then(|| FileDiagnostics { path: module.path.clone(), diags })
}

struct ModuleRewriter<'module, 'ast> {
    allocator: &'ast Allocator,
    module: &'module ModuleInfo<'ast>,
    plan: &'module Plan,
    names: &'module LoaderNames<'ast>,
    /// Where each rewrite starts, sorted, so the walk skips any node that
    /// holds none.
    sites: Vec<u32>,
}

impl<'ast> ModuleRewriter<'_, 'ast> {
    fn touches(&self, span: Span) -> bool {
        let first = self.sites.partition_point(|&start| start < span.start);
        self.sites.get(first).is_some_and(|&start| start < span.end)
    }

    /// A call of the global `require` that the bundle holds the module of,
    /// by its name or through the global table, becomes one of the loader
    /// with the module's slot.
    fn retarget_require(&self, chain: &mut SuffixedExpression<'ast>) {
        let Expression::Name(prefix) = &chain.prefix else {
            return;
        };
        // `require(path)` calls with its first suffix, and `_G.require(path)`
        // with its second.
        let callee_suffixes = usize::from(prefix.text != "require");
        let Some(Suffix::Call(written_args)) = chain.suffixes.get(callee_suffixes) else {
            return;
        };
        let args_span = written_args.span();
        let call_span = Span::new(chain.span.start, args_span.end);
        let dependencies = &self.module.dependencies;
        let Some(dependency) = dependencies
            .binary_search_by_key(&call_span.start, |dependency| dependency.call_span.start)
            .ok()
            .map(|i| &dependencies[i])
            .filter(|dependency| dependency.call_span == call_span)
        else {
            return;
        };
        let slot =
            u32::try_from(self.plan.slot(dependency)).expect("a slot is at most the module limit");
        let slot_expr =
            Expression::new_number(f64::from(slot), None, Span::default(), true, self.allocator);
        if let Some(Suffix::Call(args)) = chain.suffixes.get_mut(callee_suffixes) {
            *args = FunctionArgs::Parenthesized {
                span: args_span,
                args: List::from_array_in([slot_expr], self.allocator),
            };
        }
        if let Expression::Name(callee) = &mut chain.prefix {
            callee.text = self.names.require.into();
        }
        if callee_suffixes > 0
            && let Some(suffixes) = OneOrMore::from_list(chain.suffixes.split_off(1))
        {
            chain.suffixes = suffixes;
        }
    }

    /// The export table's field that a lowered export's reference reads or
    /// writes.
    fn export_field(&self, name: &Name<'ast>) -> Option<Box<'ast, SuffixedExpression<'ast>>> {
        self.module.exports.is_field_reference(name.span).then(|| self.table_field(*name))
    }

    /// The export table's field `name`, spanning it.
    fn table_field(&self, name: Name<'ast>) -> Box<'ast, SuffixedExpression<'ast>> {
        let table =
            Expression::new_name(self.names.exports.into(), None, name.span, self.allocator);
        let suffixes = OneOrMore::new(Suffix::Field(name), self.allocator);
        SuffixedExpression::boxed(name.span, table, suffixes, self.allocator)
    }

    /// A top-level `export` declaration loses the keyword. One whose exports
    /// live in the table assigns its fields instead.
    fn lower_export(&self, stmt: Statement<'ast>) -> Statement<'ast> {
        let exports = &self.module.exports;
        match stmt {
            Statement::TypeDeclaration(mut type_declaration) => {
                type_declaration.is_exported = false;
                Statement::TypeDeclaration(type_declaration)
            }
            Statement::LocalAssignment(mut local) if local.kind.is_exported() => {
                if !exports.is_table_statement(local.span) {
                    local.kind = if local.kind.is_const() {
                        LocalAssignmentKind::Const
                    } else {
                        LocalAssignmentKind::Local
                    };
                    return Statement::LocalAssignment(local);
                }
                let LocalAssignment { span, names, exprs, .. } = Box::unbox(local);
                let targets = List::from_iter_in(
                    names
                        .into_iter()
                        .map(|local_name| Var::Suffixed(self.table_field(local_name.name))),
                    self.allocator,
                );
                let values = if exprs.is_empty() {
                    List::from_array_in([Expression::Nil(span)], self.allocator)
                } else {
                    exprs
                };
                Statement::new_assignment(span, targets, values, self.allocator)
            }
            Statement::LocalFunction(mut function) if function.kind.is_exported() => {
                if !exports.is_table_statement(function.span) {
                    // An exported function binds a constant, and the
                    // lowered one is a plain `local function`.
                    function.kind = LocalFunctionKind::Local;
                    return Statement::LocalFunction(function);
                }
                let LocalFunction { span, attributes, name, body, .. } = Box::unbox(function);
                let table = Name::new(self.names.exports.into(), None, name.span);
                let names = List::from_array_in([table, name], self.allocator);
                let function_name = FunctionName::new(name.span, names, None);
                Statement::new_function_decl(span, attributes, function_name, body, self.allocator)
            }
            other @ (Statement::Assignment(_)
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
            | Statement::Return(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::CompoundAssignment(_)
            | Statement::Declaration(_)
            | Statement::Error(_)) => other,
        }
    }
}

impl<'ast> VisitMut<'ast> for ModuleRewriter<'_, 'ast> {
    fn visit_stmt(&mut self, stmt: &mut Statement<'ast>) {
        if !self.touches(stmt.span()) {
            return;
        }
        // `function value.f()` reads `value`, so a lowered export as its
        // root moves under the export table.
        if let Statement::FunctionDecl(function_decl) = stmt
            && let Some(root) = function_decl.name.names.first()
            && self.module.exports.is_field_reference(root.span)
        {
            let table = Name::new(self.names.exports.into(), None, root.span);
            function_decl.name.names.insert(0, table, self.allocator);
        }
        self.walk_stmt(stmt);
    }

    /// A lowered export as the prefix joins the chain.
    fn visit_suffixed_expr(&mut self, suffixed_expr: &mut SuffixedExpression<'ast>) {
        if !self.touches(suffixed_expr.span) {
            return;
        }
        self.retarget_require(suffixed_expr);
        self.walk_suffixed_expr(suffixed_expr);
        suffixed_expr.splice_prefix(self.allocator);
    }

    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        if !self.touches(expr.span()) {
            return;
        }
        if let Expression::Name(name) = expr {
            if let Some(field) = self.export_field(name) {
                *expr = Expression::Suffixed(field);
            }
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_var(&mut self, var: &mut Var<'ast>) {
        if let Var::Name(name) = var
            && let Some(field) = self.export_field(name)
        {
            *var = Var::Suffixed(field);
            return;
        }
        self.walk_var(var);
    }
}
