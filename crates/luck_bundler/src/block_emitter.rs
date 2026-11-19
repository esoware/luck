use core::mem;

use luck_allocator::{Allocator, Box, List};
use luck_ast::{
    Block, Chunk, Expression, FunctionArgs, FunctionName, GetSpan as _, LocalAssignment,
    LocalAssignmentKind, LocalFunction, LocalFunctionKind, MAX_NESTING_DEPTH, Name, OneOrMore,
    Statement, Suffix, SuffixedExpression, Var, VisitMut, spans_past_nesting_cap,
};
use luck_core::Diagnostic;
use luck_resolver::to_slash_lossy;
use luck_token::{LuaVersion, Span, chunk_header};

use crate::emitter::{self, HOLE, Key, Plan, Wrapper};
use crate::exports::contains_span;
use crate::graph::DependencyGraph;
use crate::module::ModuleInfo;

/// The bundle as a tree, and the lines its printed form opens with: the
/// entry's shebang and its Luau hot comments, which take effect only there.
///
/// Each module keeps the tree its own parse gave it, rewritten where the text
/// splices, and everything around them is the tree the text parses to, which
/// goes in `allocator` with every node the rewrites build. A module's spans
/// index its own source, so two modules' spans can overlap.
///
/// # Errors
///
/// Returns an E013 for each construct a wrapper would nest past
/// [`MAX_NESTING_DEPTH`], as [`emitter::emit`] does.
pub(crate) fn emit_chunk<'ast>(
    allocator: &'ast Allocator,
    mut graph: DependencyGraph<'ast>,
    version: LuaVersion,
) -> Result<(Chunk<'ast>, String), Vec<Diagnostic>> {
    let Some(entry) = graph.modules.get(graph.entry_id.0) else {
        let empty = Block::new(Span::default(), List::new());
        return Ok((Chunk::new(version, empty), String::new()));
    };
    let header = chunk_header(entry.source, entry.shebang, entry.hot_comments.iter().copied());
    if graph.modules.len() == 1 && entry.dependencies.is_empty() {
        let block = graph.modules.swap_remove(graph.entry_id.0).block;
        return Ok((Chunk::new(version, block), header));
    }

    let plan = Plan::new(&graph, version);
    let names = LoaderNames::new(allocator, &plan.prefix);
    let mut bodies = Vec::with_capacity(graph.modules.len());
    let mut errors = Vec::new();
    for wrapped in plan.wrapped_modules() {
        if let Some(module) = graph.modules.get_mut(wrapped.id.0) {
            let mut block = module.block.take();
            rewrite_module(allocator, &mut block, module, &plan, &names, version);
            if wrapped.wrapper.may_overflow(module) {
                errors.extend(overflow_errors(&block, module, wrapped.wrapper));
            }
            bodies.push(block);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let skeleton = allocator.alloc_str(&emitter::skeleton(&graph, &plan, version));
    let mut chunk = luck_parser::parse(allocator, skeleton, version).chunk;
    let mut filler = Filler { allocator, prefix: &plan.prefix, bodies: bodies.into_iter() };
    filler.visit_chunk(&mut chunk);
    Ok((chunk, header))
}

/// Puts each module's body, in bundle order, in place of the hole the
/// skeleton's parse holds for it.
struct Filler<'plan, 'ast, Bodies> {
    allocator: &'ast Allocator,
    prefix: &'plan str,
    bodies: Bodies,
}

impl<Bodies> Filler<'_, '_, Bodies> {
    fn is_hole(&self, stmt: &Statement<'_>) -> bool {
        let Statement::FunctionCall(call) = stmt else {
            return false;
        };
        matches!(&call.prefix, Expression::Name(name) if name.text.strip_prefix(self.prefix) == Some(HOLE))
    }
}

impl<'ast, Bodies> VisitMut<'ast> for Filler<'_, 'ast, Bodies>
where
    Bodies: Iterator<Item = Block<'ast>>,
{
    /// A block's own statements are walked before its hole is filled, so no
    /// module's are. Only an export table's `return` follows a hole, and a
    /// module with value exports has no `return` of its own.
    fn visit_block(&mut self, block: &mut Block<'ast>) {
        self.walk_block(block);
        let Some(hole) = block.stmts.iter().position(|stmt| self.is_hole(stmt)) else {
            return;
        };
        let Some(mut body) = self.bodies.next() else {
            return;
        };
        let capacity = block.stmts.len().saturating_add(body.stmts.len());
        let mut stmts = List::with_capacity_in(capacity, self.allocator);
        for (i, stmt) in mem::take(&mut block.stmts).into_iter().enumerate() {
            if i != hole {
                stmts.push(stmt, self.allocator);
                continue;
            }
            for body_stmt in mem::take(&mut body.stmts) {
                stmts.push(body_stmt, self.allocator);
            }
        }
        block.stmts = stmts;
    }
}

/// The names the rewrites give the loader's entry points and the export
/// table, in the arena.
pub(crate) struct LoaderNames<'ast> {
    require: &'ast str,
    dynamic: &'ast str,
    exports: &'ast str,
}

impl<'ast> LoaderNames<'ast> {
    pub(crate) fn new(allocator: &'ast Allocator, prefix: &str) -> Self {
        Self {
            require: allocator.alloc_concat_strs_array([prefix, "require"]),
            dynamic: allocator.alloc_concat_strs_array([prefix, "dynamic"]),
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
    plan: &Plan<'ast>,
    names: &LoaderNames<'ast>,
    version: LuaVersion,
) {
    let mut sites: Vec<u32> = module
        .dependencies
        .iter()
        .map(|dependency| dependency.call_span.start)
        .chain(module.dynamic_callees.iter().map(|callee| callee.start))
        .chain(module.exports.field_references.iter().map(|reference| reference.start))
        .collect();
    let has_exports = version.has_value_exports();
    if sites.is_empty() && !has_exports {
        return;
    }
    sites.sort_unstable();
    let mut rewriter =
        ModuleRewriter { allocator, module, plan, names, sites, key_text: String::new() };
    rewriter.visit_block(block);
    if has_exports {
        for stmt in &mut block.stmts {
            *stmt = rewriter.lower_export(stmt.take());
        }
    }
}

/// An E013 for each construct `block`, `module`'s rewritten tree, nests past
/// [`MAX_NESTING_DEPTH`] in `wrapper`.
pub(crate) fn overflow_errors(
    block: &Block<'_>,
    module: &ModuleInfo<'_>,
    wrapper: Wrapper,
) -> Vec<Diagnostic> {
    let levels = wrapper.levels();
    let file_path = to_slash_lossy(&module.path);
    spans_past_nesting_cap(block, levels)
        .into_iter()
        .map(|span| Diagnostic::e013(&file_path, span, levels, MAX_NESTING_DEPTH))
        .collect()
}

struct ModuleRewriter<'module, 'ast> {
    allocator: &'ast Allocator,
    module: &'module ModuleInfo<'ast>,
    plan: &'module Plan<'ast>,
    names: &'module LoaderNames<'ast>,
    /// Where each rewrite starts, sorted, so the walk skips any node that
    /// holds none.
    sites: Vec<u32>,
    key_text: String,
}

impl<'ast> ModuleRewriter<'_, 'ast> {
    fn touches(&self, span: Span) -> bool {
        let first = self.sites.partition_point(|&start| start < span.start);
        self.sites.get(first).is_some_and(|&start| start < span.end)
    }

    /// `require("name")` becomes a call of the loader with the module's key.
    fn retarget_require(&mut self, chain: &mut SuffixedExpression<'ast>) {
        let (Expression::Name(callee), Some(Suffix::Call(args))) =
            (&mut chain.prefix, chain.suffixes.first_mut())
        else {
            return;
        };
        let args_span = args.span();
        let call_span = callee.span.merge(args_span);
        let dependencies = &self.module.dependencies;
        let Some(dependency) = dependencies
            .binary_search_by_key(&call_span.start, |dependency| dependency.call_span.start)
            .ok()
            .and_then(|i| dependencies.get(i))
            .filter(|dependency| dependency.call_span == call_span)
        else {
            return;
        };
        let Some(key) = self.plan.key(dependency) else {
            return;
        };
        callee.text = self.names.require.into();
        let key_expr = self.key_expr(key);
        *args = FunctionArgs::Parenthesized {
            span: args_span,
            args: List::from_array_in([key_expr], self.allocator),
        };
    }

    fn key_expr(&mut self, key: Key<'_>) -> Expression<'ast> {
        self.key_text.clear();
        key.write(&mut self.key_text);
        let text = self.allocator.alloc_str(&self.key_text);
        match key {
            Key::Slot(_) => Expression::new_number(text, Span::default(), self.allocator),
            Key::Name(_) => Expression::new_string_literal(text, Span::default(), self.allocator),
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
            // Luau
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
                        .map(|attributed| Var::Suffixed(self.table_field(attributed.name))),
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
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::CompoundAssignment(_)
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
                return;
            }
            if contains_span(&self.module.dynamic_callees, name.span) {
                name.text = self.names.dynamic.into();
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
