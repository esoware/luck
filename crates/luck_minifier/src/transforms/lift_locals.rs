use rustc_hash::FxHashSet;

use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::transform::AstTransform;
use luck_token::Span;
use luck_token::token::{Token, TokenKind};

use crate::expr::ident_name;
use crate::tokens::default_span as sp;
use luck_token::CompactString;

/// Lift local declarations to function scope, eliminating redundant `local` keywords.
/// Runs post-rename: the renamer guarantees non-overlapping lifetimes for same-named
/// bindings, but parent-child slot reuse means a child local can share a name with a
/// parent local. We only lift when the name doesn't appear in any ancestor scope.
///
/// There is deliberately no per-lift byte-cost gate, even though a lone
/// lift can cost a few bytes for its hoisted head. The tail fixpoint
/// converges by annealing: each lift reshapes scopes, the next rename
/// redistributes names, and that unblocks further lifts. Gating the
/// "unprofitable" lone lifts freezes that cascade early and benchmarked
/// hundreds of bytes WORSE corpus-wide than lifting unconditionally.
pub fn lift(block: Block) -> Block {
    let mut lifter = Lifter;
    lifter.transform_block(block)
}

struct Lifter;

impl AstTransform for Lifter {
    fn walk_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        let mut param_names: FxHashSet<CompactString> = body
            .params
            .iter()
            .map(|p| ident_name(&p.name).into())
            .collect();
        // A Lua 5.5 `...name` vararg is a binding too: a hoisted
        // declaration sharing its name would capture its references.
        if let Some(vararg) = &body.vararg
            && let Some(name) = &vararg.name
        {
            param_names.insert(ident_name(name).into());
        }

        let ineligible = collect_ineligible_names(&body);

        let mut liftable_set = FxHashSet::default();
        let mut liftable_ordered = Vec::new();
        let mut scope_names = vec![param_names];
        collect_liftable(
            &body.block,
            &mut scope_names,
            false,
            &ineligible,
            &mut liftable_set,
            &mut liftable_ordered,
        );

        if liftable_set.is_empty() {
            body.block = self.transform_block(body.block);
            return body;
        }

        body.block = rewrite_block(body.block, &liftable_set, &mut Vec::new());
        body.block = self.transform_block(body.block);

        prepend_declaration(&mut body.block, &liftable_ordered);

        body
    }
}

/// Walk a block and determine which locals can be lifted.
/// A local is liftable if:
/// - its name doesn't appear in any ancestor scope (avoids clobbering parent bindings)
/// - its name is not ineligible (free in the body or captured by a closure)
/// - it has no attributes (<const>, <close>)
fn collect_liftable(
    block: &Block,
    scope_names: &mut Vec<FxHashSet<CompactString>>,
    in_loop: bool,
    ineligible: &FxHashSet<CompactString>,
    liftable: &mut FxHashSet<CompactString>,
    liftable_ordered: &mut Vec<CompactString>,
) {
    for stmt in &block.stmts {
        match stmt {
            Statement::LocalAssignment(local) => {
                // Const bindings cannot be lifted: the hoisted bare
                // declaration would lack the mandatory initializer and
                // the later write would assign to a const.
                let has_attribs = local.names.iter().any(|n| n.attrib.is_some()) || local.is_const;
                // A bare `local x` inside a loop resets to nil each
                // iteration; lifted to function scope it would keep the
                // previous iteration's value.
                let bare_in_loop = in_loop && local.exprs.is_none();
                if has_attribs || bare_in_loop {
                    for name in local.names.iter() {
                        let n = ident_name(&name.name);
                        scope_names.last_mut().unwrap().insert(n.into());
                    }
                    continue;
                }
                let names: Vec<CompactString> = local
                    .names
                    .iter()
                    .map(|attributed| ident_name(&attributed.name).into())
                    .collect();
                let mut all_safe = true;
                for name in &names {
                    if is_in_parent(scope_names, name) {
                        all_safe = false;
                        break;
                    }
                    if ineligible.contains(name) {
                        all_safe = false;
                        break;
                    }
                }
                if all_safe {
                    for name in &names {
                        if liftable.insert(name.clone()) {
                            liftable_ordered.push(name.clone());
                        }
                    }
                }
                for name in names {
                    scope_names.last_mut().unwrap().insert(name);
                }
            }
            Statement::LocalFunction(lf) => {
                let name: CompactString = ident_name(&lf.name).into();
                if !lf.is_const
                    && !is_in_parent(scope_names, &name)
                    && !ineligible.contains(&name)
                    && liftable.insert(name.clone())
                {
                    liftable_ordered.push(name.clone());
                }
                scope_names.last_mut().unwrap().insert(name);
                // don't recurse into function body - it gets its own lift pass
            }
            Statement::DoBlock(do_block) => {
                scope_names.push(FxHashSet::default());
                collect_liftable(
                    &do_block.block,
                    scope_names,
                    in_loop,
                    ineligible,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
            }
            Statement::WhileLoop(while_loop) => {
                scope_names.push(FxHashSet::default());
                collect_liftable(
                    &while_loop.block,
                    scope_names,
                    true,
                    ineligible,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
            }
            Statement::RepeatLoop(repeat_loop) => {
                scope_names.push(FxHashSet::default());
                collect_liftable(
                    &repeat_loop.block,
                    scope_names,
                    true,
                    ineligible,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
            }
            Statement::IfStatement(if_stmt) => {
                scope_names.push(FxHashSet::default());
                collect_liftable(
                    &if_stmt.block,
                    scope_names,
                    in_loop,
                    ineligible,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
                for clause in &if_stmt.elseif_clauses {
                    scope_names.push(FxHashSet::default());
                    collect_liftable(
                        &clause.block,
                        scope_names,
                        in_loop,
                        ineligible,
                        liftable,
                        liftable_ordered,
                    );
                    scope_names.pop();
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    scope_names.push(FxHashSet::default());
                    collect_liftable(
                        &else_clause.block,
                        scope_names,
                        in_loop,
                        ineligible,
                        liftable,
                        liftable_ordered,
                    );
                    scope_names.pop();
                }
            }
            Statement::NumericFor(nf) => {
                scope_names.push(FxHashSet::default());
                let var_name = ident_name(&nf.name);
                scope_names.last_mut().unwrap().insert(var_name.into());
                collect_liftable(
                    &nf.block,
                    scope_names,
                    true,
                    ineligible,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
            }
            Statement::GenericFor(gf) => {
                scope_names.push(FxHashSet::default());
                for binding in gf.names.iter() {
                    scope_names
                        .last_mut()
                        .unwrap()
                        .insert(ident_name(&binding.name).into());
                }
                collect_liftable(
                    &gf.block,
                    scope_names,
                    true,
                    ineligible,
                    liftable,
                    liftable_ordered,
                );
                scope_names.pop();
            }
            // No `local` binding to lift at this level. Function declarations
            // (including Lua 5.5 `global function`) get their own lift pass via
            // walk_function_body, so their bodies are not traversed here.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
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
    }
}

fn is_in_parent(scope_names: &[FxHashSet<CompactString>], name: &str) -> bool {
    scope_names.iter().any(|frame| frame.contains(name))
}

/// Names a hoisted declaration at the body's top must never wear, found
/// by real scope resolution over the body:
///
/// - names free in the body itself (globals and upvalues): the hoisted
///   `local` would capture every such reference in the body;
/// - names bound in the body but free in a nested closure (true
///   captures): the closure observes that binding's identity, and
///   lifting merges all same-named lifted bindings into one shared
///   function-scope binding.
///
/// Names a closure binds internally resolve inside it and block nothing,
/// which is what unlocks lifts the old mentioned-in-any-closure set
/// rejected.
fn collect_ineligible_names(body: &FunctionBody) -> FxHashSet<CompactString> {
    let mut root_frame: FxHashSet<CompactString> = body
        .params
        .iter()
        .map(|p| ident_name(&p.name).into())
        .collect();
    if let Some(vararg) = &body.vararg
        && let Some(name) = &vararg.name
    {
        root_frame.insert(ident_name(name).into());
    }
    let mut scanner = IneligibleScanner {
        frames: vec![root_frame],
        closure_floor: Vec::new(),
        ineligible: FxHashSet::default(),
    };
    scanner.scan_block(&body.block);
    scanner.ineligible
}

struct IneligibleScanner {
    /// One name set per live scope, innermost last.
    frames: Vec<FxHashSet<CompactString>>,
    /// Frame count at each enclosing nested-closure entry.
    closure_floor: Vec<usize>,
    ineligible: FxHashSet<CompactString>,
}

impl IneligibleScanner {
    fn declare(&mut self, name: &str) {
        self.frames
            .last_mut()
            .expect("root frame always present")
            .insert(name.into());
    }

    fn reference(&mut self, name: &str) {
        match self.frames.iter().rposition(|frame| frame.contains(name)) {
            None => {
                self.ineligible.insert(name.into());
            }
            Some(frame_idx) => {
                // Bound in the body proper but referenced from inside a
                // closure: a true capture.
                if let Some(&floor) = self.closure_floor.first()
                    && frame_idx < floor
                {
                    self.ineligible.insert(name.into());
                }
            }
        }
    }

    fn scan_closure(&mut self, body: &FunctionBody) {
        self.closure_floor.push(self.frames.len());
        let mut frame: FxHashSet<CompactString> = body
            .params
            .iter()
            .map(|p| ident_name(&p.name).into())
            .collect();
        if let Some(vararg) = &body.vararg
            && let Some(name) = &vararg.name
        {
            frame.insert(ident_name(name).into());
        }
        self.frames.push(frame);
        self.scan_block(&body.block);
        self.frames.pop();
        self.closure_floor.pop();
    }

    fn scan_block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.scan_stmt(stmt);
        }
        if let Some(last) = &block.last_stmt
            && let LastStatement::Return(ret) = &**last
        {
            for expr in ret.exprs.iter() {
                self.scan_expr(expr);
            }
        }
    }

    fn scan_scoped_block(&mut self, block: &Block) {
        self.frames.push(FxHashSet::default());
        self.scan_block(block);
        self.frames.pop();
    }

    fn scan_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                // RHS before LHS: `local x = x` reads the outer x.
                if let Some(exprs) = &local.exprs {
                    for expr in exprs.iter() {
                        self.scan_expr(expr);
                    }
                }
                for name_tok in local.names.iter() {
                    self.declare(ident_name(&name_tok.name));
                }
            }
            Statement::LocalFunction(local_func) => {
                self.declare(ident_name(&local_func.name));
                self.scan_closure(&local_func.body);
            }
            Statement::FunctionDecl(func_decl) => {
                self.scan_closure(&func_decl.body);
                if !func_decl.name.names.is_empty() {
                    self.reference(ident_name(&func_decl.name.names[0]));
                }
            }
            Statement::Assignment(assign) => {
                for expr in assign.values.iter() {
                    self.scan_expr(expr);
                }
                for var in assign.targets.iter() {
                    self.scan_var(var);
                }
            }
            Statement::FunctionCall(call_stmt) => {
                self.scan_call(&call_stmt.call);
            }
            Statement::DoBlock(do_block) => {
                self.scan_scoped_block(&do_block.block);
            }
            Statement::WhileLoop(while_loop) => {
                self.scan_expr(&while_loop.condition);
                self.scan_scoped_block(&while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                // The condition sees the block's locals.
                self.frames.push(FxHashSet::default());
                self.scan_block(&repeat_loop.block);
                self.scan_expr(&repeat_loop.condition);
                self.frames.pop();
            }
            Statement::IfStatement(if_stmt) => {
                self.scan_expr(&if_stmt.condition);
                self.scan_scoped_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    self.scan_expr(&clause.condition);
                    self.scan_scoped_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.scan_scoped_block(&else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                self.scan_expr(&numeric_for.start);
                self.scan_expr(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.scan_expr(step);
                }
                self.frames.push(FxHashSet::default());
                self.declare(ident_name(&numeric_for.name));
                self.scan_block(&numeric_for.block);
                self.frames.pop();
            }
            Statement::GenericFor(generic_for) => {
                for expr in generic_for.exprs.iter() {
                    self.scan_expr(expr);
                }
                self.frames.push(FxHashSet::default());
                for binding in generic_for.names.iter() {
                    self.declare(ident_name(&binding.name));
                }
                self.scan_block(&generic_for.block);
                self.frames.pop();
            }
            Statement::CompoundAssignment(compound) => {
                self.scan_var(&compound.var);
                self.scan_expr(&compound.expr);
            }
            Statement::GlobalFunction(global_func) => {
                self.scan_closure(&global_func.body);
            }
            Statement::GlobalDeclaration(global_decl) => {
                if let Some(exprs) = &global_decl.exprs {
                    for expr in exprs.iter() {
                        self.scan_expr(expr);
                    }
                }
            }
            Statement::TypeDeclaration(type_decl) => {
                if let TypeDeclarationValue::TypeFunction(func_body) = &type_decl.type_value {
                    self.scan_closure(func_body);
                }
            }
            Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_) => {}
        }
    }

    fn scan_expr(&mut self, expr: &Expression) {
        match expr {
            Expression::Var(var) => self.scan_var(var),
            Expression::FunctionCall(call) => self.scan_call(call),
            Expression::BinaryOp(binop) => {
                self.scan_expr(&binop.left);
                self.scan_expr(&binop.right);
            }
            Expression::UnaryOp(unop) => self.scan_expr(&unop.operand),
            Expression::Parenthesized(paren) => self.scan_expr(&paren.expr),
            Expression::FunctionDef(func) => self.scan_closure(&func.body),
            Expression::TableConstructor(table) => self.scan_fields(&table.fields),
            Expression::InterpolatedString(interp) => {
                for seg in &interp.segments {
                    if let Some(expr) = &seg.expr {
                        self.scan_expr(expr);
                    }
                }
            }
            Expression::IfExpression(if_expr) => {
                self.scan_expr(&if_expr.condition);
                self.scan_expr(&if_expr.then_expr);
                for clause in &if_expr.elseif_clauses {
                    self.scan_expr(&clause.condition);
                    self.scan_expr(&clause.expr);
                }
                self.scan_expr(&if_expr.else_expr);
            }
            Expression::TypeCast(cast) => self.scan_expr(&cast.expr),
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Error(_) => {}
        }
    }

    fn scan_var(&mut self, var: &Var) {
        match var {
            Var::Name(name) => self.reference(ident_name(name)),
            Var::FieldAccess(fa) => self.scan_expr(&fa.prefix),
            Var::Index(ie) => {
                self.scan_expr(&ie.prefix);
                self.scan_expr(&ie.index);
            }
        }
    }

    fn scan_call(&mut self, call: &FunctionCall) {
        self.scan_expr(&call.callee);
        match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                for arg in args.iter() {
                    self.scan_expr(arg);
                }
            }
            FunctionArgs::TableConstructor(table) => self.scan_fields(&table.fields),
            FunctionArgs::StringLiteral(_) => {}
        }
    }

    fn scan_fields(&mut self, fields: &Punctuated<Field>) {
        for field in fields.iter() {
            match field {
                Field::Bracketed { key, value, .. } => {
                    self.scan_expr(key);
                    self.scan_expr(value);
                }
                Field::Named { value, .. } => self.scan_expr(value),
                Field::Positional { value, .. } => self.scan_expr(value),
            }
        }
    }
}

/// Rewrite a block: convert lifted `local X=Y` to `X=Y`, remove bare `local X`.
/// `shadowed` carries names bound by NON-lifted declarations (loop control
/// variables, kept locals) on the path here: a declaration whose name is
/// currently shadowed must stay a `local` - rewriting it into an
/// assignment would rebind it to the shadowing binding, not the hoisted
/// one (e.g. a renamed shadow inside `for l = ...` assigning the loop var).
fn rewrite_block(
    block: Block,
    liftable: &FxHashSet<CompactString>,
    shadowed: &mut Vec<FxHashSet<CompactString>>,
) -> Block {
    let mut new_stmts: Vec<Statement> = Vec::new();
    shadowed.push(FxHashSet::default());

    for stmt in block.stmts {
        match stmt {
            Statement::LocalAssignment(local) => {
                let names: Vec<CompactString> = local
                    .names
                    .iter()
                    .map(|attributed| ident_name(&attributed.name).into())
                    .collect();
                let all_lifted = names.iter().all(|n| liftable.contains(n))
                    && !names
                        .iter()
                        .any(|n| shadowed.iter().any(|frame| frame.contains(n)));

                if all_lifted {
                    if let Some(exprs) = local.exprs {
                        // local a,b=X,Y -> a,b=X,Y
                        let targets = Punctuated::from_items(
                            local
                                .names
                                .items
                                .into_iter()
                                .map(|attributed| Var::Name(attributed.name))
                                .collect(),
                        );
                        new_stmts.push(Statement::Assignment(Box::new(Assignment {
                            span: local.span,
                            targets,
                            values: exprs,
                        })));
                    }
                    // bare `local X` with no values: just drop it
                } else {
                    for name in names {
                        shadowed
                            .last_mut()
                            .expect("frame pushed above")
                            .insert(name);
                    }
                    new_stmts.push(Statement::LocalAssignment(local));
                }
            }
            Statement::LocalFunction(lf) => {
                let name: CompactString = ident_name(&lf.name).into();
                if liftable.contains(&name) && !shadowed.iter().any(|frame| frame.contains(&name)) {
                    // local function f(x)...end -> f=function(x)...end
                    let func_expr = Expression::FunctionDef(Box::new(FunctionDef {
                        span: lf.span,
                        attributes: lf.attributes,
                        body: lf.body,
                    }));
                    let body_block = rewrite_block_in_funcdef(func_expr, liftable);
                    new_stmts.push(Statement::Assignment(Box::new(Assignment {
                        span: lf.span,
                        targets: Punctuated::from_item(Var::Name(lf.name)),
                        values: Punctuated::from_item(body_block),
                    })));
                } else {
                    shadowed
                        .last_mut()
                        .expect("frame pushed above")
                        .insert(name);
                    new_stmts.push(Statement::LocalFunction(lf));
                }
            }
            Statement::DoBlock(mut do_block) => {
                do_block.block = rewrite_block(do_block.block, liftable, shadowed);
                new_stmts.push(Statement::DoBlock(do_block));
            }
            Statement::WhileLoop(mut wl) => {
                wl.block = rewrite_block(wl.block, liftable, shadowed);
                new_stmts.push(Statement::WhileLoop(wl));
            }
            Statement::RepeatLoop(mut rl) => {
                rl.block = rewrite_block(rl.block, liftable, shadowed);
                new_stmts.push(Statement::RepeatLoop(rl));
            }
            Statement::IfStatement(mut if_stmt) => {
                if_stmt.block = rewrite_block(if_stmt.block, liftable, shadowed);
                if_stmt.elseif_clauses = if_stmt
                    .elseif_clauses
                    .into_iter()
                    .map(|mut clause| {
                        clause.block = rewrite_block(clause.block, liftable, shadowed);
                        clause
                    })
                    .collect();
                if_stmt.else_clause = if_stmt.else_clause.map(|mut ec| {
                    ec.block = rewrite_block(ec.block, liftable, shadowed);
                    ec
                });
                new_stmts.push(Statement::IfStatement(if_stmt));
            }
            Statement::NumericFor(mut nf) => {
                let mut frame = FxHashSet::default();
                frame.insert(CompactString::from(ident_name(&nf.name)));
                shadowed.push(frame);
                nf.block = rewrite_block(nf.block, liftable, shadowed);
                shadowed.pop();
                new_stmts.push(Statement::NumericFor(nf));
            }
            Statement::GenericFor(mut gf) => {
                let mut frame = FxHashSet::default();
                for binding in gf.names.iter() {
                    frame.insert(CompactString::from(ident_name(&binding.name)));
                }
                shadowed.push(frame);
                gf.block = rewrite_block(gf.block, liftable, shadowed);
                shadowed.pop();
                new_stmts.push(Statement::GenericFor(gf));
            }
            // No lifted local to rewrite here. Function declarations (including
            // Lua 5.5 `global function`) get their own lift pass, so their
            // bodies pass through unchanged.
            stmt @ (Statement::Assignment(_)
            | Statement::FunctionCall(_)
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
            | Statement::Error(_)) => new_stmts.push(stmt),
        }
    }

    shadowed.pop();
    Block {
        span: block.span,
        stmts: new_stmts,
        last_stmt: block.last_stmt,
    }
}

fn rewrite_block_in_funcdef(expr: Expression, _liftable: &FxHashSet<CompactString>) -> Expression {
    // don't rewrite inside nested function defs - they get their own lift pass
    expr
}

fn prepend_declaration(block: &mut Block, names: &[CompactString]) {
    if names.is_empty() {
        return;
    }
    let name_tokens: Vec<AttributedName> = names
        .iter()
        .map(|n| AttributedName {
            name: Token::new(TokenKind::Identifier(n.clone()), Span::default()),
            type_annotation: None,
            attrib: None,
        })
        .collect();
    let names = Punctuated::from_items(name_tokens.into_iter().collect());

    block.stmts.insert(
        0,
        Statement::LocalAssignment(Box::new(LocalAssignment {
            span: sp(),
            names,
            exprs: None,
            is_const: false,
        })),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_core::types::LuaTarget;

    fn minify(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse failed");
        let block = crate::transforms::rename_locals::rename(result.block, LuaTarget::Lua54, true);
        let block = lift(block);
        luck_codegen::compact(&block, source)
    }

    fn reparses(source: &str) -> bool {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        result.errors.is_empty()
    }

    #[test]
    fn lifts_nested_locals() {
        let result = minify(concat!(
            "local function f(x)\n",
            "  local a = 1\n",
            "  if x then\n",
            "    local b = 2\n",
            "    print(a, b)\n",
            "  end\n",
            "  return a\n",
            "end\n",
            "return f\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        // Should have fewer `local` keywords than original
    }

    #[test]
    fn does_not_lift_parent_shadowed() {
        // After rename, if inner and outer share a name, inner must keep `local`
        let result = minify(concat!(
            "local function f()\n",
            "  local a = 1\n",
            "  do\n",
            "    local b = 2\n", // might get same name as a after rename
            "    print(b)\n",
            "  end\n",
            "  return a\n", // a is still needed after do-block
            "end\n",
            "return f\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
    }

    #[test]
    fn does_not_lift_closure_in_loop() {
        let result = minify(concat!(
            "local t = {}\n",
            "for i = 1, 10 do\n",
            "  local x = i\n",
            "  t[i] = function() return x end\n",
            "end\n",
            "return t\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        // x must keep its `local` to preserve per-iteration binding
        assert!(
            result.contains("local"),
            "closure-captured loop local must keep local: {result}"
        );
    }

    #[test]
    fn lifts_loop_local_without_closure() {
        let result = minify(concat!(
            "local s = 0\n",
            "for i = 1, 10 do\n",
            "  local x = i * 2\n",
            "  s = s + x\n",
            "end\n",
            "return s\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
    }

    fn apply_lift_only(source: &str) -> String {
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse failed");
        let block = lift(result.block);
        luck_codegen::compact(&block, source)
    }

    #[test]
    fn does_not_lift_over_free_reference() {
        // `u[k]` reads the module upvalue; the inner `local u` shadows it
        // only from its declaration point on. Hoisting `local u` to the
        // body top would capture the earlier read (observed miscompiling
        // roact's Config:set validation path).
        let result = apply_lift_only(concat!(
            "local u = {}\n",
            "local function f(k)\n",
            "  if u[k] == nil then\n",
            "    local u = 1\n",
            "    print(u)\n",
            "  end\n",
            "end\n",
            "return f\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            result.contains("local u=1") || result.contains("local u = 1"),
            "decl sharing a free-referenced name must stay local: {result}"
        );
    }

    #[test]
    fn capture_outside_loop_still_blocks_lift() {
        let result = apply_lift_only(concat!(
            "local function f()\n",
            "  do\n",
            "    local x = 1\n",
            "    g = function() return x end\n",
            "  end\n",
            "  do\n",
            "    local x = 2\n",
            "    h = function() return x end\n",
            "  end\n",
            "end\n",
            "return f\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        // Hoisting would merge both x bindings into one, letting the two
        // closures observe each other's writes.
        assert!(
            result.matches("local x").count() == 2,
            "captured locals must not merge into one hoisted slot: {result}"
        );
    }

    #[test]
    fn closure_internal_names_do_not_block_lift() {
        // The closure binds its own `x`; that must not veto lifting the
        // unrelated do-block `x` (the old mentioned-in-any-closure set did).
        let result = apply_lift_only(concat!(
            "local function f()\n",
            "  local c = function() local x = 1 return x end\n",
            "  do\n",
            "    local x = 2\n",
            "    print(x)\n",
            "  end\n",
            "  return c\n",
            "end\n",
            "return f\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        assert!(
            !result.contains("local x=2") && !result.contains("local x = 2"),
            "closure-internal name must not block the lift: {result}"
        );
    }

    #[test]
    fn named_vararg_blocks_same_named_lift() {
        let source = concat!(
            "local function f(...args)\n",
            "  do\n",
            "    local args = 1\n",
            "    print(args)\n",
            "  end\n",
            "end\n",
            "return f\n",
        );
        let result = luck_parser::parse(source, luck_token::LuaVersion::Lua55);
        assert!(result.errors.is_empty(), "parse failed");
        let block = lift(result.block);
        let output = luck_codegen::compact(&block, source);
        assert!(
            output.contains("local args=1") || output.contains("local args = 1"),
            "decl sharing the vararg name must stay local: {output}"
        );
    }

    #[test]
    fn shadowed_declaration_is_never_rewritten_to_assignment() {
        // The lift rewrite matches by NAME: a shadowing declaration that
        // shares a lifted name must stay a `local`, or its rewrite would
        // rebind to the shadowing binding (here, the for control var).
        let result = minify(concat!(
            "function W.new(c)\n",
            "  for l = 1, 3 do\n",
            "    if c then\n",
            "      local l = f()\n",
            "      print(l)\n",
            "    end\n",
            "  end\n",
            "  while c do\n",
            "    local l = g()\n",
            "    print(l)\n",
            "  end\n",
            "end\n",
        ));
        assert!(reparses(&result), "Parse errors\nOutput: {result}");
        // The declaration under the for-var shadow must survive as a
        // `local` (the while-loop one may be lifted).
        assert!(
            result.contains("local l=f()") || result.contains("local l = f()"),
            "shadowed decl must stay local: {result}"
        );
    }
}
