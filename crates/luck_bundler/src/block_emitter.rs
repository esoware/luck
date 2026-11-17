use core::mem;

use luck_allocator::{Allocator, Box, List};
use luck_ast::{
    Assignment, Block, Expression, FunctionArgs, FunctionDecl, FunctionName, Literal,
    LocalAssignment, LocalAssignmentKind, LocalFunction, LocalFunctionKind, MAX_NESTING_DEPTH,
    Name, OneOrMore, Statement, Suffix, SuffixedExpression, Var, VisitorMut,
    spans_past_nesting_cap,
};
use luck_core::Diagnostic;
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
pub(crate) fn emit_block<'ast>(
    allocator: &'ast Allocator,
    mut graph: DependencyGraph<'ast>,
    version: LuaVersion,
) -> Result<(Block<'ast>, String), Vec<Diagnostic>> {
    let Some(entry) = graph.modules.get(graph.entry_id.0) else {
        let empty = Block { span: Span::default(), stmts: List::new(), last_stmt: None };
        return Ok((empty, String::new()));
    };
    let header = chunk_header(entry.source, entry.shebang, entry.hot_comments.iter().copied());
    if graph.modules.len() == 1 && entry.dependencies.is_empty() {
        let block = graph.modules.swap_remove(graph.entry_id.0).block;
        return Ok((block, header));
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
    let mut block = luck_parser::parse(allocator, skeleton, version).block;
    let mut filler = Filler { allocator, prefix: &plan.prefix, bodies: bodies.into_iter() };
    filler.visit_block(&mut block);
    Ok((block, header))
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

impl<'ast, Bodies> VisitorMut<'ast> for Filler<'_, 'ast, Bodies>
where
    Bodies: Iterator<Item = Block<'ast>>,
{
    /// A block's own statements are walked before its hole is filled, so no
    /// module's are.
    fn visit_block(&mut self, block: &mut Block<'ast>) {
        self.walk_block(block);
        let hole = block.stmts.iter().position(|stmt| self.is_hole(stmt));
        // Only the text needs the `;` that keeps a body opening with `(` from
        // continuing the wrapper's statement.
        let has_empty = block.stmts.iter().any(|stmt| matches!(stmt, Statement::EmptyStatement(_)));
        if hole.is_none() && !has_empty {
            return;
        }
        let mut body = hole.and_then(|_| self.bodies.next());
        let capacity = block
            .stmts
            .len()
            .saturating_add(body.as_ref().map_or(0, |filling| filling.stmts.len()));
        let mut stmts = List::with_capacity_in(capacity, self.allocator);
        for (i, stmt) in mem::take(&mut block.stmts).into_iter().enumerate() {
            if Some(i) == hole
                && let Some(Block { stmts: body_stmts, last_stmt, .. }) = body.take()
            {
                for body_stmt in body_stmts {
                    stmts.push(body_stmt, self.allocator);
                }
                block.last_stmt = block.last_stmt.take().or(last_stmt);
                continue;
            }
            if !matches!(stmt, Statement::EmptyStatement(_)) {
                stmts.push(stmt, self.allocator);
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
    spans_past_nesting_cap(block, levels)
        .into_iter()
        .map(|span| Diagnostic::e013(&module.path, span, levels, MAX_NESTING_DEPTH))
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
        callee.text = self.names.require;
        let key_expr = self.key_expr(key);
        *args = FunctionArgs::Parenthesized {
            span: args_span,
            args: List::from_array_in([key_expr], self.allocator),
        };
    }

    fn key_expr(&mut self, key: Key<'_>) -> Expression<'ast> {
        self.key_text.clear();
        key.write(&mut self.key_text);
        let literal = Box::new_in(
            Literal { text: self.allocator.alloc_str(&self.key_text), span: Span::default() },
            self.allocator,
        );
        match key {
            Key::Slot(_) => Expression::Number(literal),
            Key::Name(_) => Expression::StringLiteral(literal),
        }
    }

    /// The export table's field that a lowered export's reference reads or
    /// writes.
    fn export_field(&self, name: &Name<'ast>) -> Option<Box<'ast, SuffixedExpression<'ast>>> {
        self.module
            .exports
            .is_field_reference(name.span)
            .then(|| Box::new_in(self.table_field(*name), self.allocator))
    }

    /// The export table's field `name`, spanning it.
    fn table_field(&self, name: Name<'ast>) -> SuffixedExpression<'ast> {
        let table = Name { text: self.names.exports, span: name.span };
        SuffixedExpression {
            span: name.span,
            prefix: Expression::Name(Box::new_in(table, self.allocator)),
            suffixes: OneOrMore::new(Suffix::Field(name), self.allocator),
        }
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
                    names.into_iter().map(|attributed| {
                        Var::Suffixed(Box::new_in(
                            self.table_field(attributed.name),
                            self.allocator,
                        ))
                    }),
                    self.allocator,
                );
                let values = if exprs.is_empty() {
                    List::from_array_in([Expression::Nil(span)], self.allocator)
                } else {
                    exprs
                };
                Statement::Assignment(Box::new_in(
                    Assignment { span, targets, values },
                    self.allocator,
                ))
            }
            Statement::LocalFunction(mut function) if function.kind.is_exported() => {
                if !exports.is_table_statement(function.span) {
                    // An exported function binds a constant, and the
                    // lowered one is a plain `local function`.
                    function.kind = LocalFunctionKind::Local;
                    return Statement::LocalFunction(function);
                }
                let LocalFunction { span, attributes, name, body, .. } = Box::unbox(function);
                let table = Name { text: self.names.exports, span: name.span };
                let function_name = FunctionName {
                    span: name.span,
                    names: List::from_array_in([table, name], self.allocator),
                    method: None,
                };
                let declaration = FunctionDecl { span, attributes, name: function_name, body };
                Statement::FunctionDecl(Box::new_in(declaration, self.allocator))
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
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::Error(_)) => other,
        }
    }
}

impl<'ast> VisitorMut<'ast> for ModuleRewriter<'_, 'ast> {
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
            let table = Name { text: self.names.exports, span: root.span };
            function_decl.name.names.insert(0, table, self.allocator);
        }
        self.walk_stmt(stmt);
    }

    /// A lowered export as the prefix joins the chain.
    fn visit_suffixed(&mut self, chain: &mut SuffixedExpression<'ast>) {
        if !self.touches(chain.span) {
            return;
        }
        self.retarget_require(chain);
        self.walk_suffixed(chain);
        chain.splice_prefix(self.allocator);
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
                name.text = self.names.dynamic;
            }
            return;
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &mut Var<'ast>) {
        if let Var::Name(name) = target
            && let Some(field) = self.export_field(name)
        {
            *target = Var::Suffixed(field);
            return;
        }
        self.walk_assignment_target(target);
    }
}
