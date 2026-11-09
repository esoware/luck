use luck_ast::{
    Assignment, AttributedName, Block, Expression, FunctionBody, FunctionDef, LocalAssignment,
    LocalFunction, Punctuated, Statement, Transform, Type, Var, Visitor,
};
use luck_token::{CompactString, LuaVersion, Span};
use rustc_hash::FxHashSet;

use crate::binding::{has_fixed_binding, identifier_name, identifier_token};

/// Lifts local declarations to function scope, dropping redundant `local`
/// keywords.
///
/// Runs after rename, which gives same-named bindings non-overlapping
/// lifetimes. Slot reuse can still give a child local its parent's name, so a
/// name already bound in an enclosing scope is left alone.
///
/// There's deliberately no per-lift size check, even though a lone lift can
/// cost a few bytes for the hoisted declaration. Each lift reshapes scopes,
/// the next rename redistributes names, and that unblocks further lifts.
/// Skipping the unprofitable lone lifts stops that cascade early, and it
/// benchmarked hundreds of bytes worse across the corpus than lifting
/// everything.
pub(crate) fn lift(block: Block, version: LuaVersion) -> Block {
    let mut lifter = Lifter { version };
    lifter.transform_block(block)
}

struct Lifter {
    version: LuaVersion,
}

impl Transform for Lifter {
    fn transform_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        let ineligible_names = collect_ineligible_names(&body, self.version);

        let mut collector = LiftableCollector {
            scope_names: vec![param_names(&body)],
            ineligible_names: &ineligible_names,
            liftable_names: FxHashSet::default(),
            ordered_liftable_names: Vec::new(),
            version: self.version,
        };
        collector.collect_block(&body.block);
        let LiftableCollector { liftable_names, ordered_liftable_names, .. } = collector;

        if liftable_names.is_empty() {
            body.block = self.transform_block(body.block);
            return body;
        }

        let mut rewriter = LiftRewriter {
            liftable_names: &liftable_names,
            scope_names: Vec::new(),
            version: self.version,
        };
        body.block = rewriter.rewrite_block(body.block);
        body.block = self.transform_block(body.block);

        prepend_declaration(&mut body.block, &ordered_liftable_names);

        body
    }
}

/// Collects the locals in a function body that can be lifted.
///
/// A name qualifies when it isn't already bound in this or an enclosing scope
/// of the body (rename can give a child local its parent's name) and isn't
/// ineligible (see `collect_ineligible_names`). A `local` statement also has
/// to pass `can_lift_assignment`, which rules out attributes such as
/// `<const>` and `<close>`.
struct LiftableCollector<'names> {
    scope_names: Vec<FxHashSet<CompactString>>,
    ineligible_names: &'names FxHashSet<CompactString>,
    liftable_names: FxHashSet<CompactString>,
    ordered_liftable_names: Vec<CompactString>,
    version: LuaVersion,
}

impl LiftableCollector<'_> {
    fn declare(&mut self, name: &str) {
        if let Some(frame) = self.scope_names.last_mut() {
            frame.insert(name.into());
        }
    }

    fn is_safe_to_lift(&self, name: &str) -> bool {
        !self.scope_names.iter().any(|frame| frame.contains(name))
            && !self.ineligible_names.contains(name)
    }

    fn mark_liftable(&mut self, name: &str) {
        if self.liftable_names.insert(name.into()) {
            self.ordered_liftable_names.push(name.into());
        }
    }

    fn collect_block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.collect_stmt(stmt);
        }
    }

    fn collect_block_in_frame(&mut self, block: &Block, frame: FxHashSet<CompactString>) {
        self.scope_names.push(frame);
        self.collect_block(block);
        self.scope_names.pop();
    }

    fn collect_scoped_block(&mut self, block: &Block) {
        self.collect_block_in_frame(block, FxHashSet::default());
    }

    fn collect_local(&mut self, local: &LocalAssignment) {
        let names = local.names.items.iter().map(|attributed| identifier_name(&attributed.name));
        if can_lift_assignment(local, self.version)
            && names.clone().all(|name| self.is_safe_to_lift(name))
        {
            for name in names.clone() {
                self.mark_liftable(name);
            }
        }
        for name in names {
            self.declare(name);
        }
    }

    fn collect_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::LocalAssignment(local) => self.collect_local(local),
            Statement::LocalFunction(local_function) => {
                let name = identifier_name(&local_function.name);
                if !local_function.is_const
                    && !local_function.is_exported
                    && self.is_safe_to_lift(name)
                {
                    self.mark_liftable(name);
                }
                self.declare(name);
                // The function body gets its own lift pass, so stop here.
            }
            Statement::DoBlock(do_block) => self.collect_scoped_block(&do_block.block),
            Statement::WhileLoop(while_loop) => self.collect_scoped_block(&while_loop.block),
            Statement::RepeatLoop(repeat_loop) => self.collect_scoped_block(&repeat_loop.block),
            Statement::IfStatement(if_stmt) => {
                self.collect_scoped_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    self.collect_scoped_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.collect_scoped_block(&else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                let frame = FxHashSet::from_iter([identifier_name(&numeric_for.name).into()]);
                self.collect_block_in_frame(&numeric_for.block, frame);
            }
            Statement::GenericFor(generic_for) => {
                let frame = generic_for
                    .names
                    .iter()
                    .map(|param| identifier_name(&param.name).into())
                    .collect();
                self.collect_block_in_frame(&generic_for.block, frame);
            }
            // Nothing to lift at this level. Function bodies get their own
            // pass through `transform_function_body`.
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

fn can_lift_assignment(local: &LocalAssignment, version: LuaVersion) -> bool {
    // A bare `local x` resets x to nil, which a lifted one wouldn't once a
    // sibling has used the shared slot. A type annotation would be lost,
    // since the hoisted declaration is a bare `local x`.
    local.exprs.is_some()
        && !has_fixed_binding(local, version)
        && local.names.iter().all(|attributed| attributed.type_annotation.is_none())
        && local.names.iter().enumerate().all(|(index, attributed)| {
            local.names.iter().take(index).all(|previous| {
                identifier_name(&previous.name) != identifier_name(&attributed.name)
            })
        })
}

/// The names `body`'s parameters bind. A Lua 5.5 `...name` vararg is one too,
/// and a hoisted declaration sharing its name would capture its references.
fn param_names(body: &FunctionBody) -> FxHashSet<CompactString> {
    let mut names: FxHashSet<CompactString> =
        body.params.iter().map(|param| identifier_name(&param.name).into()).collect();
    if let Some(vararg) = &body.vararg
        && let Some(name) = &vararg.name
    {
        names.insert(identifier_name(name).into());
    }
    names
}

/// Names a declaration hoisted to the top of `body` must never wear, found by
/// resolving scopes over the body:
///
/// - names free in the body (globals and upvalues), since the hoisted `local`
///   would capture every reference to them;
/// - names bound in the body but captured by a nested closure, since lifting
///   merges same-named bindings into one function-scope local that the
///   closure would then share;
/// - `_ENV` on targets where it's an upvalue.
///
/// A name a closure binds itself resolves inside that closure, so merely
/// mentioning a name in a closure doesn't block its lift.
fn collect_ineligible_names(body: &FunctionBody, version: LuaVersion) -> FxHashSet<CompactString> {
    let mut scanner = IneligibleScanner {
        frames: vec![param_names(body)],
        closure_floors: Vec::new(),
        ineligible_names: if version.has_env_upvalue() {
            FxHashSet::from_iter([CompactString::from("_ENV")])
        } else {
            FxHashSet::default()
        },
    };
    scanner.visit_block(&body.block);
    scanner.ineligible_names
}

struct IneligibleScanner {
    /// One name set per live scope, innermost last.
    frames: Vec<FxHashSet<CompactString>>,
    /// Frame count at each enclosing nested-closure entry.
    closure_floors: Vec<usize>,
    ineligible_names: FxHashSet<CompactString>,
}

impl IneligibleScanner {
    fn declare(&mut self, name: &str) {
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(name.into());
        }
    }

    fn reference(&mut self, name: &str) {
        match self.frames.iter().rposition(|frame| frame.contains(name)) {
            None => {
                self.ineligible_names.insert(name.into());
            }
            Some(frame_index) => {
                // Bound in the body itself but referenced from inside a
                // closure, so it's captured.
                if let Some(&floor) = self.closure_floors.first()
                    && frame_index < floor
                {
                    self.ineligible_names.insert(name.into());
                }
            }
        }
    }

    fn scan_scoped_block(&mut self, block: &Block) {
        self.frames.push(FxHashSet::default());
        self.visit_block(block);
        self.frames.pop();
    }
}

impl<'ast> Visitor<'ast> for IneligibleScanner {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                // RHS before LHS: `local x = x` reads the outer x.
                if let Some(exprs) = &local.exprs {
                    for expr in exprs.iter() {
                        self.visit_expr(expr);
                    }
                }
                for attributed in local.names.iter() {
                    self.declare(identifier_name(&attributed.name));
                }
            }
            Statement::LocalFunction(local_function) => {
                self.declare(identifier_name(&local_function.name));
                self.visit_function_body(&local_function.body);
            }
            Statement::FunctionDecl(function_declaration) => {
                self.visit_function_body(&function_declaration.body);
                if let Some(root_name) = function_declaration.name.names.first() {
                    self.reference(identifier_name(root_name));
                }
            }
            Statement::DoBlock(do_block) => self.scan_scoped_block(&do_block.block),
            Statement::WhileLoop(while_loop) => {
                self.visit_expr(&while_loop.condition);
                self.scan_scoped_block(&while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                // The condition sees the block's locals.
                self.frames.push(FxHashSet::default());
                self.visit_block(&repeat_loop.block);
                self.visit_expr(&repeat_loop.condition);
                self.frames.pop();
            }
            Statement::IfStatement(if_stmt) => {
                self.visit_expr(&if_stmt.condition);
                self.scan_scoped_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    self.visit_expr(&clause.condition);
                    self.scan_scoped_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.scan_scoped_block(&else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                self.visit_expr(&numeric_for.start);
                self.visit_expr(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.visit_expr(step);
                }
                self.frames.push(FxHashSet::default());
                self.declare(identifier_name(&numeric_for.name));
                self.visit_block(&numeric_for.block);
                self.frames.pop();
            }
            Statement::GenericFor(generic_for) => {
                for expr in generic_for.exprs.iter() {
                    self.visit_expr(expr);
                }
                self.frames.push(FxHashSet::default());
                for param in generic_for.names.iter() {
                    self.declare(identifier_name(&param.name));
                }
                self.visit_block(&generic_for.block);
                self.frames.pop();
            }
            // These bind nothing in the body's scopes. A `global function`
            // or `type function` body is reached as a closure.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::CompoundAssignment(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalDeclaration(_)
            | Statement::TypeDeclaration(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_) => self.walk_stmt(stmt),
        }
    }

    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(name) = var {
            self.reference(identifier_name(name));
        }
        self.walk_var(var);
    }

    /// A nested closure, which sees the body's bindings as upvalues.
    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.closure_floors.push(self.frames.len());
        self.frames.push(param_names(body));
        self.visit_block(&body.block);
        self.frames.pop();
        self.closure_floors.pop();
    }

    // Types never run, so a name inside one is never captured.
    fn visit_type(&mut self, _ty: &'ast Type) {}
}

/// Turns lifted `local` declarations into plain assignments.
///
/// Matching is by name, so it tracks every declaration in scope, lifted or
/// not. A declaration under a same-named binding stays `local`, since its
/// assignment would write to that binding instead.
struct LiftRewriter<'names> {
    liftable_names: &'names FxHashSet<CompactString>,
    scope_names: Vec<FxHashSet<CompactString>>,
    version: LuaVersion,
}

impl LiftRewriter<'_> {
    fn is_shadowed(&self, name: &str) -> bool {
        self.scope_names.iter().any(|frame| frame.contains(name))
    }

    fn rewrite_block(&mut self, block: Block) -> Block {
        self.rewrite_block_in_frame(block, FxHashSet::default())
    }

    fn rewrite_block_in_frame(&mut self, block: Block, frame: FxHashSet<CompactString>) -> Block {
        self.scope_names.push(frame);
        let stmts = block.stmts.into_iter().map(|stmt| self.rewrite_stmt(stmt)).collect();
        self.scope_names.pop();
        Block { span: block.span, stmts, last_stmt: block.last_stmt }
    }

    fn rewrite_local(&mut self, local: Box<LocalAssignment>) -> Statement {
        let names = local.names.items.iter().map(|attributed| identifier_name(&attributed.name));
        let is_lifted = can_lift_assignment(&local, self.version)
            && names.clone().all(|name| self.liftable_names.contains(name))
            && !names.clone().any(|name| self.is_shadowed(name));

        if let Some(frame) = self.scope_names.last_mut() {
            frame.extend(names.map(CompactString::from));
        }
        // local a,b=X,Y -> a,b=X,Y. `can_lift_assignment` has already
        // rejected bare declarations, so `exprs` is always set here.
        if is_lifted && let Some(values) = local.exprs {
            let targets = Punctuated::from_items(
                local
                    .names
                    .items
                    .into_iter()
                    .map(|attributed| Var::Name(attributed.name))
                    .collect(),
            );
            return Statement::Assignment(Box::new(Assignment {
                span: local.span,
                targets,
                values,
            }));
        }
        Statement::LocalAssignment(local)
    }

    fn rewrite_local_function(&mut self, local_function: Box<LocalFunction>) -> Statement {
        let name = identifier_name(&local_function.name);
        let is_lifted = !local_function.is_const
            && !local_function.is_exported
            && self.liftable_names.contains(name)
            && !self.is_shadowed(name);
        if let Some(frame) = self.scope_names.last_mut() {
            frame.insert(name.into());
        }
        if !is_lifted {
            return Statement::LocalFunction(local_function);
        }
        // local function f(x)...end -> f=function(x)...end
        let function_expr = Expression::FunctionDef(Box::new(FunctionDef {
            span: local_function.span,
            attributes: local_function.attributes,
            body: local_function.body,
        }));
        Statement::Assignment(Box::new(Assignment {
            span: local_function.span,
            targets: Punctuated::from_item(Var::Name(local_function.name)),
            values: Punctuated::from_item(function_expr),
        }))
    }

    fn rewrite_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::LocalAssignment(local) => self.rewrite_local(local),
            Statement::LocalFunction(local_function) => self.rewrite_local_function(local_function),
            Statement::DoBlock(mut do_block) => {
                do_block.block = self.rewrite_block(do_block.block);
                Statement::DoBlock(do_block)
            }
            Statement::WhileLoop(mut while_loop) => {
                while_loop.block = self.rewrite_block(while_loop.block);
                Statement::WhileLoop(while_loop)
            }
            Statement::RepeatLoop(mut repeat_loop) => {
                repeat_loop.block = self.rewrite_block(repeat_loop.block);
                Statement::RepeatLoop(repeat_loop)
            }
            Statement::IfStatement(mut if_stmt) => {
                if_stmt.block = self.rewrite_block(if_stmt.block);
                if_stmt.elseif_clauses = if_stmt
                    .elseif_clauses
                    .into_iter()
                    .map(|mut clause| {
                        clause.block = self.rewrite_block(clause.block);
                        clause
                    })
                    .collect();
                if_stmt.else_clause = if_stmt.else_clause.map(|mut else_clause| {
                    else_clause.block = self.rewrite_block(else_clause.block);
                    else_clause
                });
                Statement::IfStatement(if_stmt)
            }
            Statement::NumericFor(mut numeric_for) => {
                let frame = FxHashSet::from_iter([identifier_name(&numeric_for.name).into()]);
                numeric_for.block = self.rewrite_block_in_frame(numeric_for.block, frame);
                Statement::NumericFor(numeric_for)
            }
            Statement::GenericFor(mut generic_for) => {
                let frame = generic_for
                    .names
                    .iter()
                    .map(|param| identifier_name(&param.name).into())
                    .collect();
                generic_for.block = self.rewrite_block_in_frame(generic_for.block, frame);
                Statement::GenericFor(generic_for)
            }
            // Nothing to rewrite at this level. Function bodies get their own
            // pass through `transform_function_body`.
            other @ (Statement::Assignment(_)
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
            | Statement::Error(_)) => other,
        }
    }
}

fn prepend_declaration(block: &mut Block, names: &[CompactString]) {
    if names.is_empty() {
        return;
    }
    let attributed_names: Vec<AttributedName> = names
        .iter()
        .map(|name| AttributedName {
            name: identifier_token(name.clone()),
            type_annotation: None,
            attribute: None,
        })
        .collect();

    block.stmts.insert(
        0,
        Statement::LocalAssignment(Box::new(LocalAssignment {
            span: Span::default(),
            names: Punctuated::from_items(attributed_names),
            exprs: None,
            is_const: false,
            is_exported: false,
        })),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};
    use crate::transforms::rename_locals::rename;

    /// Lifts `source` without renaming it first, and checks it reparses.
    fn apply(source: &str) -> String {
        let output = run_pass(source, LuaVersion::Lua54, |block| lift(block, LuaVersion::Lua54));
        assert_reparses(&output, LuaVersion::Lua54);
        output
    }

    /// Renames, then lifts, as the pipeline's tail does, and checks the
    /// result reparses.
    fn rename_and_lift(source: &str) -> String {
        let output = run_pass(source, LuaVersion::Lua54, |block| {
            lift(rename(block, LuaVersion::Lua54, true), LuaVersion::Lua54)
        });
        assert_reparses(&output, LuaVersion::Lua54);
        output
    }

    #[test]
    fn lifts_nested_locals() {
        let result = apply(concat!(
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
        assert!(
            result.contains("local a,b"),
            "both declarations should hoist to one function-head local: {result}"
        );
        assert!(
            !result.contains("local b"),
            "the nested declaration should lose its `local`: {result}"
        );
    }

    #[test]
    fn does_not_lift_parent_shadowed() {
        // When rename gives inner and outer the same name, the inner
        // declaration must keep its `local`.
        let result = rename_and_lift(concat!(
            "local function f()\n",
            "  local a = 1\n",
            "  do\n",
            "    local b = 2\n", // rename gives this `a`'s name
            "    print(b)\n",
            "  end\n",
            "  return a\n",
            "end\n",
            "return f\n",
        ));
        assert!(
            result.contains("do local l=2"),
            "the shadowed inner declaration must keep its `local`: {result}"
        );
    }

    #[test]
    fn does_not_lift_closure_in_loop() {
        // Only function bodies get lifted, so the loop needs a wrapper.
        let result = apply(concat!(
            "local function f()\n",
            "  local t = {}\n",
            "  for i = 1, 10 do\n",
            "    local x = i\n",
            "    t[i] = function() return x end\n",
            "  end\n",
            "  return t\n",
            "end\n",
            "return f\n",
        ));
        assert!(
            result.contains("local x=i"),
            "closure-captured loop local must keep its per-iteration binding: {result}"
        );
        assert!(result.contains("local t t="), "the uncaptured local should still hoist: {result}");
    }

    #[test]
    fn lifts_loop_local_without_closure() {
        let result = apply(concat!(
            "local function f()\n",
            "  local s = 0\n",
            "  for i = 1, 10 do\n",
            "    local x = i * 2\n",
            "    s = s + x\n",
            "  end\n",
            "  return s\n",
            "end\n",
            "return f\n",
        ));
        assert!(
            result.contains("local s,x"),
            "an uncaptured loop local should hoist to the function head: {result}"
        );
        assert!(
            !result.contains("local x"),
            "the loop-body declaration should lose its `local`: {result}"
        );
    }

    #[test]
    fn does_not_lift_over_free_reference() {
        // `u[k]` reads the outer `u`, and the inner `local u` shadows it only
        // from its declaration on. Hoisting `local u` to the top of the body
        // would capture that read, which miscompiled roact's `Config:set`.
        let result = apply(concat!(
            "local u = {}\n",
            "local function f(k)\n",
            "  if u[k] == nil then\n",
            "    local u = 1\n",
            "    print(u)\n",
            "  end\n",
            "end\n",
            "return f\n",
        ));
        assert!(
            result.contains("local u=1") || result.contains("local u = 1"),
            "decl sharing a free-referenced name must stay local: {result}"
        );
    }

    #[test]
    fn capture_outside_loop_still_blocks_lift() {
        let result = apply(concat!(
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
        // unrelated do-block `x`.
        let result = apply(concat!(
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
        let output = run_pass(source, LuaVersion::Lua55, |block| lift(block, LuaVersion::Lua54));
        assert!(
            output.contains("local args=1") || output.contains("local args = 1"),
            "decl sharing the vararg name must stay local: {output}"
        );
    }

    #[test]
    fn shadowed_declaration_is_never_rewritten_to_assignment() {
        // The rewrite matches by name, so a declaration under a same-named
        // binding (here the `for` variable) must stay `local`, or its
        // assignment would write to that binding.
        let result = rename_and_lift(concat!(
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
        // The declaration under the `for` variable must stay `local`. The
        // one in the while loop may be lifted.
        assert!(
            result.contains("local l=f()") || result.contains("local l = f()"),
            "shadowed decl must stay local: {result}"
        );
    }

    #[test]
    fn bug_hoisted_declaration_stays_within_the_local_limit() {
        let body = (0..205u32)
            .map(|i| format!("do local v{i} = g() print(v{i}) end\n"))
            .collect::<Vec<_>>()
            .concat();
        let result = apply(&format!("local function w()\n{body}end\nreturn w\n"));
        let hoisted = result.split("local ").nth(2).expect("hoisted declaration");
        let (names, _) = hoisted.split_once(' ').expect("names end at the next statement");
        let name_count = names.split(',').count();
        assert!(
            name_count <= 200,
            "a function may hold 200 locals at once, but {name_count} were hoisted"
        );
    }
}
