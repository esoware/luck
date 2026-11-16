use luck_ast::{
    Assignment, Block, CallSuffix, Expression, FunctionArgs, FunctionDecl, FunctionName, Literal,
    LocalAssignment, LocalAssignmentKind, LocalFunction, LocalFunctionKind, OneOrMore, Punctuated,
    Statement, Suffix, SuffixedExpression, Transform, Var,
};
use luck_core::Diagnostic;
use luck_token::{CompactString, LuaVersion, Span, Token, TokenKind, chunk_header};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::emitter::{self, HOLE, Item, Key, Plan};
use crate::exports::ExportPlan;
use crate::graph::DependencyGraph;
use crate::module::ModuleInfo;

/// The bundle as a tree, and the lines its printed form opens with: the
/// entry's shebang and its Luau hot comments, which take effect only there.
///
/// Each module keeps the tree its own parse gave it, rewritten where the text
/// splices, and everything around them is the tree the text parses to. A
/// module's spans index its own source, so two modules' spans can overlap.
///
/// # Errors
///
/// Returns the errors [`Plan::new`] does.
pub(crate) fn emit_block(
    mut graph: DependencyGraph,
    version: LuaVersion,
) -> Result<(Block, String), Vec<Diagnostic>> {
    let Some(entry) = graph.modules.get(graph.entry_id.0) else {
        let empty = Block { span: Span::default(), stmts: Vec::new(), last_stmt: None };
        return Ok((empty, String::new()));
    };
    let header = chunk_header(&entry.source, entry.shebang, entry.hot_comments.iter().copied());
    if graph.modules.len() == 1 && entry.dependencies.is_empty() {
        let block = graph.modules.swap_remove(graph.entry_id.0).block;
        return Ok((block, header));
    }

    let plan = Plan::new(&graph, version)?;
    let skeleton = emitter::skeleton(&graph, entry, &plan, version);
    let mut bodies = Vec::new();
    for item in &plan.items {
        if let Item::Module(wrapped) = item
            && let Some(module) = graph.modules.get_mut(wrapped.id.0)
        {
            let block = module.block.take();
            bodies.push(rewrite_module(block, module, &plan, &wrapped.exports, version));
        }
    }
    let mut filler = Filler { hole: format!("{}{HOLE}", plan.prefix), bodies: bodies.into_iter() };
    let block = filler.transform_block(luck_parser::parse(skeleton, version).block);
    Ok((block, header))
}

/// Puts each module's body, in bundle order, in place of the hole the
/// skeleton's parse holds for it.
struct Filler<Bodies> {
    hole: String,
    bodies: Bodies,
}

impl<Bodies> Filler<Bodies> {
    fn is_hole(&self, stmt: &Statement) -> bool {
        let Statement::FunctionCall(call) = stmt else {
            return false;
        };
        matches!(&call.prefix, Expression::Name(name) if name.identifier() == Some(&self.hole))
    }
}

impl<Bodies> Transform for Filler<Bodies>
where
    Bodies: Iterator<Item = Block>,
{
    /// A block's own statements are walked before its hole is filled, so no
    /// module's are.
    fn transform_block(&mut self, block: Block) -> Block {
        let mut walked = self.walk_block(block);
        // Only the text needs the `;` that keeps a body opening with `(` from
        // continuing the wrapper's statement.
        walked.stmts.retain(|stmt| !matches!(stmt, Statement::EmptyStatement(_)));
        if let Some(at) = walked.stmts.iter().position(|stmt| self.is_hole(stmt))
            && let Some(body) = self.bodies.next()
        {
            walked.stmts.splice(at..=at, body.stmts);
            walked.last_stmt = walked.last_stmt.or(body.last_stmt);
        }
        walked
    }
}

fn key_expr(key: &Key) -> Expression {
    let literal = Literal { text: key.text(), span: Span::default() };
    match key {
        Key::Slot(_) => Expression::Number(literal),
        Key::Name(_) => Expression::StringLiteral(literal),
    }
}

/// `block`, `module`'s, as the bundle holds it: each require retargeted at
/// the loader, and each export lowered as `exports` plans.
pub(crate) fn rewrite_module(
    block: Block,
    module: &ModuleInfo,
    plan: &Plan,
    exports: &ExportPlan,
    version: LuaVersion,
) -> Block {
    let mut rewriter = ModuleRewriter {
        require: identifier(format!("{}require", plan.prefix)),
        dynamic: identifier(format!("{}dynamic", plan.prefix)),
        exports: identifier(format!("{}exports", plan.prefix)),
        require_keys: module
            .dependencies
            .iter()
            .filter_map(|dependency| Some((dependency.call_span, plan.key(dependency)?)))
            .collect(),
        dynamic_callees: module.dynamic_callees.iter().copied().collect(),
        field_references: &exports.field_references,
    };
    let mut rewritten = rewriter.transform_block(block);
    if version.has_value_exports() {
        for stmt in &mut rewritten.stmts {
            *stmt = rewriter.lower_export(stmt.take(), exports);
        }
    }
    rewritten
}

fn identifier(name: String) -> TokenKind {
    TokenKind::Identifier(CompactString::from(name))
}

struct ModuleRewriter<'plan> {
    require: TokenKind,
    dynamic: TokenKind,
    exports: TokenKind,
    /// Each static require's call span and the key of the module it loads.
    require_keys: FxHashMap<Span, Key>,
    dynamic_callees: FxHashSet<Span>,
    field_references: &'plan FxHashSet<Span>,
}

impl ModuleRewriter<'_> {
    /// `require("name")` becomes a call of the loader with the module's key.
    fn retarget_require(&self, chain: &mut SuffixedExpression) {
        let (Expression::Name(callee), Some(Suffix::Call(CallSuffix::Plain(args)))) =
            (&mut chain.prefix, chain.suffixes.first_mut())
        else {
            return;
        };
        let args_span = args.span();
        if let Some(key) = self.require_keys.get(&callee.span.merge(args_span)) {
            callee.kind = self.require.clone();
            *args = FunctionArgs::Parenthesized {
                span: args_span,
                args: Punctuated::from_item(key_expr(key)),
            };
        }
    }

    /// The export table's field that a lowered export's reference reads or
    /// writes.
    fn export_field(&self, name: &Token) -> Option<Box<SuffixedExpression>> {
        self.field_references.contains(&name.span).then(|| Box::new(self.table_field(name.clone())))
    }

    /// A top-level `export` declaration loses the keyword. One whose exports
    /// live in the table assigns its fields instead.
    fn lower_export(&self, stmt: Statement, exports: &ExportPlan) -> Statement {
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
                let LocalAssignment { span, names, exprs, .. } = *local;
                let targets = names
                    .into_items()
                    .into_iter()
                    .map(|attributed| Var::Suffixed(Box::new(self.table_field(attributed.name))))
                    .collect();
                let values = exprs.unwrap_or_else(|| Punctuated::from_item(Expression::Nil(span)));
                Statement::Assignment(Box::new(Assignment {
                    span,
                    targets: Punctuated::from_items(targets),
                    values,
                }))
            }
            Statement::LocalFunction(mut function) if function.kind.is_exported() => {
                if !exports.is_table_statement(function.span) {
                    // An exported function binds a constant, and the
                    // lowered one is a plain `local function`.
                    function.kind = LocalFunctionKind::Local;
                    return Statement::LocalFunction(function);
                }
                let LocalFunction { span, attributes, name, body, .. } = *function;
                let exports_name = Token::new(self.exports.clone(), name.span);
                let function_name =
                    FunctionName { span: name.span, names: vec![exports_name, name], method: None };
                Statement::FunctionDecl(Box::new(FunctionDecl {
                    span,
                    attributes,
                    name: function_name,
                    body,
                }))
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

    /// The export table's field `name`, spanning it.
    fn table_field(&self, name: Token) -> SuffixedExpression {
        SuffixedExpression {
            span: name.span,
            prefix: Expression::Name(Token::new(self.exports.clone(), name.span)),
            suffixes: OneOrMore::new(Suffix::Field(name)),
        }
    }
}

impl Transform for ModuleRewriter<'_> {
    fn transform_stmt(&mut self, mut stmt: Statement) -> Statement {
        // `function value.f()` reads `value`, so a lowered export as its
        // root moves under the export table.
        if let Statement::FunctionDecl(function_decl) = &mut stmt
            && let Some(root) = function_decl.name.names.first()
            && self.field_references.contains(&root.span)
        {
            let exports_name = Token::new(self.exports.clone(), root.span);
            function_decl.name.names.insert(0, exports_name);
        }
        self.walk_stmt(stmt)
    }

    /// A lowered export as the prefix joins the chain.
    fn transform_suffixed(
        &mut self,
        mut chain: Box<SuffixedExpression>,
    ) -> Box<SuffixedExpression> {
        self.retarget_require(&mut chain);
        let mut walked = self.walk_suffixed(chain);
        walked.splice_prefix();
        walked
    }

    fn transform_expr(&mut self, expr: Expression) -> Expression {
        if let Expression::Name(name) = &expr {
            if let Some(field) = self.export_field(name) {
                return Expression::Suffixed(field);
            }
            if self.dynamic_callees.contains(&name.span) {
                return Expression::Name(Token::new(self.dynamic.clone(), name.span));
            }
        }
        self.walk_expr(expr)
    }

    fn transform_assignment_target(&mut self, target: Var) -> Var {
        if let Var::Name(name) = &target
            && let Some(field) = self.export_field(name)
        {
            return Var::Suffixed(field);
        }
        self.walk_assignment_target(target)
    }
}
