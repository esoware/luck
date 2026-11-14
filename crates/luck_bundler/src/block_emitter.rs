use core::mem;

use luck_ast::{
    Assignment, Block, Expression, FunctionArgs, FunctionDecl, FunctionName, FunctionSignature,
    Literal, LocalAssignment, LocalFunction, OneOrMore, Punctuated, Statement, Suffix,
    SuffixedExpression, Synth, Transform, Var,
};
use luck_core::Diagnostic;
use luck_token::{CompactString, LuaVersion, Span, Token, TokenKind};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::emitter::{Binding, Item, Key, Plan, Registration, WrappedModule};
use crate::exports::ExportPlan;
use crate::graph::DependencyGraph;
use crate::loader::{dynamic_entry_text, export_runtime_text, loader_text};
use crate::module::ModuleInfo;

/// The bundle as a tree, for a caller that minifies it rather than reading
/// it, and the lines its printed form opens with: the entry's shebang and
/// its Luau hot comments, which only take effect there.
///
/// It holds what the bundle's text does, so it parses from that text, but no
/// comments, and every module keeps the tree its own parse gave it, rewritten
/// where the text splices.
///
/// # Errors
///
/// Returns the errors [`Plan::new`] does.
pub(crate) fn emit_block(
    mut graph: DependencyGraph,
    version: LuaVersion,
) -> Result<(Block, String), Vec<Diagnostic>> {
    let Some(entry) = graph.modules.get(graph.entry_id.0) else {
        return Ok((
            Block { span: Span::default(), stmts: Vec::new(), last_stmt: None },
            String::new(),
        ));
    };
    let header = header(entry);
    let is_lone = graph.modules.len() == 1 && entry.dependencies.is_empty();
    if is_lone {
        let block = graph.modules.swap_remove(graph.entry_id.0).block;
        return Ok((block, header));
    }

    let mut plan = Plan::new(&graph, version)?;
    let prefix = plan.prefix.clone();
    let mut prelude = loader_text(&prefix, version, plan.is_loader_global);
    if plan.has_dynamic_entry {
        prelude.push_str(&dynamic_entry_text(&prefix, plan.is_loader_global));
    }
    if plan.has_export_runtime {
        prelude.push_str(&export_runtime_text(&prefix));
    }
    // The loader is a fixed template, and its text, which the plain bundle
    // writes, is the one place its code is spelled out.
    let mut stmts = luck_parser::parse(prelude, version).block.stmts;

    let synth = Synth::new().with_version(version);
    let names = GlueNames::new(&prefix);
    for item in mem::take(&mut plan.items) {
        match item {
            Item::Module(wrapped) => {
                if let Some(module) = graph.modules.get_mut(wrapped.id.0) {
                    let block = module.block.take();
                    let body = rewrite_module(block, module, &plan, &wrapped.exports, version);
                    stmts.extend(wrap(&synth, &names, &wrapped, body));
                }
            }
            Item::Registration(registration) => {
                stmts.push(registration_stmt(&synth, &names, &registration));
            }
        }
    }
    let entry_call = synth.call(synth.name_expr(&names.entry), vec![synth.vararg()]);
    let last_stmt = Some(Box::new(synth.return_(vec![entry_call])));
    Ok((Block { span: Span::default(), stmts, last_stmt }, header))
}

fn header(entry: &ModuleInfo) -> String {
    let mut header = String::new();
    if let Some(shebang) = entry.shebang {
        // A trailing CR would pair with this line break into a CRLF.
        header.push_str(shebang.text(&entry.source).trim_end_matches('\r'));
        header.push('\n');
    }
    for hot_comment in &entry.hot_comments {
        header.push_str(hot_comment.text(&entry.source).trim_end());
        header.push('\n');
    }
    header
}

/// The names of the loader's tables and functions the glue reads.
struct GlueNames {
    modules: String,
    paths: String,
    entry: String,
    exporting: String,
    exports: String,
    freeze: String,
}

impl GlueNames {
    fn new(prefix: &str) -> Self {
        Self {
            modules: format!("{prefix}modules"),
            paths: format!("{prefix}paths"),
            entry: format!("{prefix}entry"),
            exporting: format!("{prefix}exporting"),
            exports: format!("{prefix}exports"),
            freeze: format!("{prefix}freeze"),
        }
    }
}

fn key_expr(key: &Key) -> Expression {
    let literal = Literal { text: key.text(), span: Span::default() };
    match key {
        Key::Slot(_) => Expression::Number(literal),
        Key::Name(_) => Expression::StringLiteral(literal),
    }
}

fn slot_expr(slot: usize) -> Expression {
    key_expr(&Key::Slot(slot))
}

fn registration_stmt(synth: &Synth, names: &GlueNames, registration: &Registration) -> Statement {
    let module_slot = |key: &Key| synth.index_var(synth.name_expr(&names.modules), key_expr(key));
    match registration {
        Registration::Exporting { slot } => {
            let module = synth.index(synth.name_expr(&names.modules), slot_expr(*slot));
            let exporting =
                synth.call(synth.name_expr(&names.exporting), vec![slot_expr(*slot), module]);
            synth.assign(vec![module_slot(&Key::Slot(*slot))], vec![exporting])
        }
        Registration::Entry { key, exporting_slot } => {
            let entry = synth.name_expr(&names.entry);
            let registered = match exporting_slot {
                Some(slot) => {
                    synth.call(synth.name_expr(&names.exporting), vec![slot_expr(*slot), entry])
                }
                None => entry,
            };
            synth.assign(vec![module_slot(key)], vec![registered])
        }
        Registration::Alias { alias, primary } => {
            let shared = synth.index(synth.name_expr(&names.modules), key_expr(primary));
            synth.assign(vec![module_slot(alias)], vec![shared])
        }
        Registration::Path { name, path } => {
            let target = synth.index_var(synth.name_expr(&names.paths), key_expr(name));
            let literal =
                Expression::StringLiteral(Literal { text: path.clone(), span: Span::default() });
            synth.assign(vec![target], vec![literal])
        }
    }
}

/// `body` in the function the bundle registers a module as.
fn wrap(
    synth: &Synth,
    names: &GlueNames,
    wrapped: &WrappedModule,
    mut body: Block,
) -> Vec<Statement> {
    let shape = wrapped.wrapper;
    let exports = &wrapped.exports;
    let mut opening = Vec::new();
    if shape.has_private_env {
        opening.push(synth.assign(vec![synth.name_var("_ENV")], vec![synth.name_expr("_ENV")]));
    }
    if shape.restores_arg {
        let global_arg = synth.field(synth.name_expr("_G"), "arg");
        opening.push(synth.assign(vec![synth.name_var("arg")], vec![global_arg]));
    }
    if exports.has_table() {
        opening.push(synth.local(&[names.exports.as_str()], vec![synth.table(Vec::new())]));
    }
    if !opening.is_empty() {
        opening.append(&mut body.stmts);
        body.stmts = opening;
    }
    if exports.has_values {
        let freeze = synth.name_expr(&names.freeze);
        let frozen = if exports.has_table() {
            for name in &exports.local_exports {
                let field = synth.field_var(synth.name_expr(&names.exports), name);
                body.stmts.push(synth.assign(vec![field], vec![synth.name_expr(name)]));
            }
            synth.name_expr(&names.exports)
        } else {
            synth.record(
                exports
                    .local_exports
                    .iter()
                    .map(|name| (name.as_str(), synth.name_expr(name)))
                    .collect(),
            )
        };
        body.last_stmt = Some(Box::new(synth.return_(vec![synth.call(freeze, vec![frozen])])));
    }

    let signature = FunctionSignature {
        vararg: shape.has_vararg_param.then(|| synth.vararg_param(None)),
        ..FunctionSignature::default()
    };
    let function = synth.function_def_full(Vec::new(), signature, body);
    let binding = match &wrapped.binding {
        Binding::Slot(key) => {
            let slot = synth.index_var(synth.name_expr(&names.modules), key_expr(key));
            synth.assign(vec![slot], vec![function])
        }
        Binding::Entry if shape.has_private_env => {
            synth.assign(vec![synth.name_var(&names.entry)], vec![function])
        }
        Binding::Entry => synth.local(&[names.entry.as_str()], vec![function]),
    };
    if !shape.has_private_env {
        return vec![binding];
    }
    // A real chunk's `_ENV` is its own and its upvalue 1, which 5.2's
    // `module()` sets, so the block gives the module a private one.
    let private_env = synth.local(&["_ENV"], vec![synth.name_expr("_ENV")]);
    let scope = synth.do_block(synth.block(vec![private_env, binding], None));
    match wrapped.binding {
        Binding::Entry => vec![synth.local(&[names.entry.as_str()], Vec::new()), scope],
        Binding::Slot(_) => vec![scope],
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
        let (Expression::Name(callee), Some(Suffix::Call(call))) =
            (&mut chain.prefix, chain.suffixes.first_mut())
        else {
            return;
        };
        if call.method.is_some() {
            return;
        }
        let args_span = call.args.span();
        if let Some(key) = self.require_keys.get(&callee.span.merge(args_span)) {
            callee.kind = self.require.clone();
            call.args = FunctionArgs::Parenthesized {
                span: args_span,
                args: Punctuated::from_item(key_expr(key)),
            };
        }
    }

    /// The export table's field that a lowered export's reference reads or
    /// writes.
    fn export_field(&self, name: &Token) -> Option<Box<SuffixedExpression>> {
        self.field_references.contains(&name.span).then(|| self.table_field(name.clone()))
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
            Statement::LocalAssignment(mut local) if local.is_exported => {
                if !exports.is_table_statement(local.span) {
                    local.is_exported = false;
                    return Statement::LocalAssignment(local);
                }
                let LocalAssignment { span, names, exprs, .. } = *local;
                let targets = names
                    .into_items()
                    .into_iter()
                    .map(|attributed| Var::Suffixed(self.table_field(attributed.name)))
                    .collect();
                let values = exprs.unwrap_or_else(|| Punctuated::from_item(Expression::Nil(span)));
                Statement::Assignment(Box::new(Assignment {
                    span,
                    targets: Punctuated::from_items(targets),
                    values,
                }))
            }
            Statement::LocalFunction(mut function) if function.is_exported => {
                if !exports.is_table_statement(function.span) {
                    // An exported function binds a constant, and the
                    // lowered one is a plain `local function`.
                    function.is_exported = false;
                    function.is_const = false;
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
    fn table_field(&self, name: Token) -> Box<SuffixedExpression> {
        Box::new(SuffixedExpression {
            span: name.span,
            prefix: Expression::Name(Token::new(self.exports.clone(), name.span)),
            suffixes: OneOrMore::new(Suffix::Field(name)),
        })
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
