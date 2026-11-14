use core::{iter, mem};

use luck_ast::{
    BinaryOp, Block, DoBlock, ElseClause, Expression, FunctionBody, IfStatement, LastStatement,
    LocalAssignment, LocalFunction, NumericFor, OneOrMore, Punctuated, RepeatLoop, Statement,
    Transform, UnaryOp, Var, Visitor,
};
use luck_token::{BinOp, CompactString, LuaVersion, Span, Token, UnOp};
use rustc_hash::FxHashMap;

use crate::binding::{has_attributed_or_env_name, identifier_name, is_env_binding};
use crate::expr::{
    binds_looser_than_unary, extract_boolean, fits_at_depth, is_nil, is_pure_expr,
    is_same_pure_expr, parenthesize, truncate_multi_value,
};

pub(crate) fn remove(mut block: Block, version: LuaVersion) -> Block {
    loop {
        let Some(live_names) = scan(&block, version) else {
            return block;
        };
        let mut remover = DeadCodeRemover { live_names, version, has_changed: false, depth: 0 };
        block = remover.transform_block(block);
        if !remover.has_changed {
            return block;
        }
    }
}

/// Nested loops and function bodies aren't searched, since their exits can't
/// reach this loop.
fn has_loop_exit(block: &Block) -> bool {
    if matches!(
        block.last_stmt.as_deref(),
        Some(LastStatement::Break(_) | LastStatement::Continue(_))
    ) {
        return true;
    }
    block.stmts.iter().any(stmt_has_loop_exit)
}

fn stmt_has_loop_exit(stmt: &Statement) -> bool {
    match stmt {
        // Lua 5.2+
        Statement::Break(_) => true,
        Statement::DoBlock(do_block) => has_loop_exit(&do_block.block),
        Statement::IfStatement(if_stmt) => {
            has_loop_exit(&if_stmt.block)
                || if_stmt.elseif_clauses.iter().any(|clause| has_loop_exit(&clause.block))
                || if_stmt.else_clause.as_ref().is_some_and(|clause| has_loop_exit(&clause.block))
        }
        Statement::WhileLoop(_)
        | Statement::RepeatLoop(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::GlobalFunction(_)
        | Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::LocalAssignment(_)
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalStar(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => false,
    }
}

/// The names the code a round keeps uses, or `None` when the round would
/// leave `block` as it is, so it needn't run.
///
/// Names match by name and not by binding. A `local` the round drops when
/// none of its names is live owns the uses in what goes with it, which count
/// only once one of its names is live. So a run of locals each used only by
/// the next goes in one round, where counting every use would take a round
/// per link.
fn scan(block: &Block, version: LuaVersion) -> Option<LiveNames> {
    let mut scanner = UseScanner {
        version,
        nodes_by_name: FxHashMap::default(),
        nodes: Vec::new(),
        edges: Vec::new(),
        pending: Vec::new(),
        owner: None,
        has_rewrite: false,
    };
    scanner.visit_block(block);
    scanner.propagate();
    let UseScanner { nodes_by_name, nodes, has_rewrite, .. } = scanner;
    (has_rewrite || nodes.iter().any(|node| node.is_local && !node.is_live))
        .then_some(LiveNames { nodes_by_name, nodes })
}

struct LiveNames {
    nodes_by_name: FxHashMap<CompactString, usize>,
    nodes: Vec<UseNode>,
}

impl LiveNames {
    fn contains(&self, name: &str) -> bool {
        self.nodes_by_name
            .get(name)
            .and_then(|&node| self.nodes.get(node))
            .is_some_and(|node| node.is_live)
    }
}

/// A name, live when code the round keeps uses it, or a droppable `local`,
/// live when one of its names is.
struct UseNode {
    is_live: bool,
    is_local: bool,
    first_edge: Option<usize>,
}

/// What a node keeps once it's live: a name keeps the `local`s that declare
/// it, and a `local` the names it uses. A node's edges are linked through
/// `next`.
#[derive(Clone, Copy)]
struct UseEdge {
    target: usize,
    next: Option<usize>,
}

struct UseScanner {
    version: LuaVersion,
    nodes_by_name: FxHashMap<CompactString, usize>,
    nodes: Vec<UseNode>,
    edges: Vec<UseEdge>,
    /// Live nodes whose edges aren't followed yet.
    pending: Vec<usize>,
    /// The innermost droppable `local` whose dropped parts the walk is in.
    owner: Option<usize>,
    /// Whether the walk has met a rewrite other than an unused `local`. Every
    /// rewrite `DeadCodeRemover` makes has to set it, or a round with nothing
    /// else to do skips that rewrite.
    has_rewrite: bool,
}

impl UseScanner {
    fn push_node(&mut self, is_local: bool) -> usize {
        let node = self.nodes.len();
        self.nodes.push(UseNode { is_live: false, is_local, first_edge: None });
        node
    }

    fn name_node(&mut self, name: &str) -> usize {
        if let Some(&node) = self.nodes_by_name.get(name) {
            return node;
        }
        let node = self.push_node(false);
        self.nodes_by_name.insert(name.into(), node);
        node
    }

    fn is_live(&self, node: usize) -> bool {
        self.nodes.get(node).is_some_and(|state| state.is_live)
    }

    /// Makes `target` live once `node` is. Only a node that isn't live yet
    /// gains an edge, so one that goes live with none never needs following.
    fn keep(&mut self, node: usize, target: usize) {
        if self.is_live(node) {
            self.mark_live(target);
            return;
        }
        let edge = self.edges.len();
        if !self.is_live(target)
            && let Some(source) = self.nodes.get_mut(node)
        {
            self.edges.push(UseEdge { target, next: source.first_edge.replace(edge) });
        }
    }

    fn mark_live(&mut self, node: usize) {
        if let Some(state) = self.nodes.get_mut(node)
            && !mem::replace(&mut state.is_live, true)
            && state.first_edge.is_some()
        {
            self.pending.push(node);
        }
    }

    fn propagate(&mut self) {
        while let Some(node) = self.pending.pop() {
            let mut edge = self.nodes.get(node).and_then(|state| state.first_edge);
            while let Some(&UseEdge { target, next }) = edge.and_then(|index| self.edges.get(index))
            {
                self.mark_live(target);
                edge = next;
            }
        }
    }

    fn record_use(&mut self, name: &str) {
        let node = self.name_node(name);
        match self.owner {
            Some(owner) => self.keep(owner, node),
            None => self.mark_live(node),
        }
    }

    /// Makes a droppable `local` the owner, returning the one it replaces.
    fn open_owner<'name, Names>(&mut self, names: Names) -> Option<usize>
    where
        Names: Iterator<Item = &'name Token>,
    {
        let owner = self.push_node(true);
        for name in names {
            let name_node = self.name_node(identifier_name(name));
            self.keep(name_node, owner);
        }
        self.owner.replace(owner)
    }

    fn visit_droppable_local(&mut self, local: &LocalAssignment) {
        let outer_owner = self.open_owner(local.names.iter().map(|attributed| &attributed.name));
        let owner = self.owner;
        self.walk_attributed_names(&local.names);
        for expr in local.exprs.iter().flat_map(Punctuated::iter) {
            // A call stays when the `local` goes.
            self.owner = if is_call(expr) { outer_owner } else { owner };
            self.visit_expr(expr);
        }
        self.owner = outer_owner;
    }
}

impl<'ast> Visitor<'ast> for UseScanner {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Name(name) = expr {
            self.record_use(identifier_name(name));
        }
        if !self.has_rewrite
            && let Expression::BinaryOp(binary_op) = expr
        {
            self.has_rewrite = is_redundant_ternary(binary_op);
        }
        self.walk_expr(expr);
    }

    fn visit_assignment_target(&mut self, target: &'ast Var) {
        if let Var::Name(name) = target {
            self.record_use(identifier_name(name));
        }
        self.walk_assignment_target(target);
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.has_rewrite = self.has_rewrite || ends_in_bare_return(&body.block);
        self.walk_function_body(body);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        self.has_rewrite = self.has_rewrite || is_rewritten(stmt);
        match stmt {
            Statement::LocalAssignment(local)
                if !matches!(unused_local_action(local, self.version), DeadLocalAction::Keep) =>
            {
                self.visit_droppable_local(local);
            }
            Statement::LocalFunction(local_function)
                if is_droppable_function(local_function, self.version) =>
            {
                let outer_owner = self.open_owner(iter::once(&local_function.name));
                self.walk_stmt(stmt);
                self.owner = outer_owner;
            }
            // `function M.f()` reads `M` and `function f()` writes `f`, and
            // neither root is a name expression or an assignment target.
            Statement::FunctionDecl(function_decl) => {
                if let Some(root) = function_decl.name.names.first() {
                    self.record_use(identifier_name(root));
                }
                self.walk_stmt(stmt);
            }
            Statement::LocalAssignment(_)
            | Statement::LocalFunction(_)
            | Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => self.walk_stmt(stmt),
        }
    }
}

/// Removes unused locals and dead branches in one traversal.
///
/// The two rewrites compose per statement, and both `remove()`'s loop and
/// the pipeline's outer loop run to a fixpoint, so interleaving them reaches
/// the same result as separate rebuilds in half the traversals.
///
struct DeadCodeRemover {
    live_names: LiveNames,
    version: LuaVersion,
    has_changed: bool,
    /// Levels of nesting open where the walk is.
    depth: usize,
}

fn constant_truthiness(expr: &Expression) -> Option<bool> {
    if let Some(value) = extract_boolean(expr) {
        return Some(value);
    }
    if is_nil(expr) {
        return Some(false);
    }
    if matches!(expr, Expression::Number(_) | Expression::Integer(_) | Expression::StringLiteral(_))
    {
        return Some(true);
    }
    None
}

fn is_dead_branch(stmt: &Statement) -> bool {
    match stmt {
        Statement::WhileLoop(while_loop) => {
            constant_truthiness(&while_loop.condition) == Some(false)
        }
        Statement::IfStatement(if_stmt) => {
            // An empty-body `if` can only go when evaluating the condition is
            // side-effect free: `if f() then end` calls f.
            let has_no_effect = is_empty_block(&if_stmt.block) && is_pure_expr(&if_stmt.condition);
            if_stmt.elseif_clauses.is_empty()
                && if_stmt.else_clause.is_none()
                && (constant_truthiness(&if_stmt.condition) == Some(false) || has_no_effect)
        }
        // `x = x` stays. Without binding resolution this pass can't prove `x`
        // is a local, and on a global under a metatabled environment it fires
        // __index and __newindex.
        Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::DoBlock(_)
        | Statement::RepeatLoop(_)
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
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => false,
    }
}

/// Whether the round rewrites `stmt` itself, other than to drop an unused
/// `local`.
///
/// It may claim an `if` that `simplify_if` then finds too deep to negate,
/// which costs only a round that changes nothing.
fn is_rewritten(stmt: &Statement) -> bool {
    match stmt {
        Statement::IfStatement(if_stmt) => {
            iter::once(&if_stmt.condition)
                .chain(if_stmt.elseif_clauses.iter().map(|clause| &clause.condition))
                .any(|condition| constant_truthiness(condition).is_some())
                || (if_stmt.elseif_clauses.is_empty()
                    && if_stmt.else_clause.is_some()
                    && (is_empty_block(&if_stmt.block) || is_negation(&if_stmt.condition)))
                || is_dead_branch(stmt)
        }
        Statement::WhileLoop(_) => is_dead_branch(stmt),
        Statement::RepeatLoop(repeat_loop) => runs_once(repeat_loop),
        Statement::NumericFor(numeric_for) => has_default_step(numeric_for),
        Statement::LocalAssignment(local) => is_nil_initialized(local),
        Statement::DoBlock(do_block) => is_empty_block(&do_block.block),
        Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalFunction(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => false,
    }
}

const fn is_empty_block(block: &Block) -> bool {
    block.stmts.is_empty() && block.last_stmt.is_none()
}

fn is_negation(expr: &Expression) -> bool {
    matches!(expr, Expression::UnaryOp(unop) if unop.op == UnOp::Not)
}

/// `repeat ... until true` runs once, so it can become a `do` block unless
/// its body has its own `break` or `continue`, which a `do` block would
/// rebind to an outer loop (or make invalid).
fn runs_once(repeat_loop: &RepeatLoop) -> bool {
    constant_truthiness(&repeat_loop.condition) == Some(true) && !has_loop_exit(&repeat_loop.block)
}

/// A step of 1 is the default.
fn has_default_step(numeric_for: &NumericFor) -> bool {
    matches!(&numeric_for.step, Some(Expression::Number(literal)) if literal.text == "1")
}

/// `local x = nil` can become `local x`, since nil is the default. Const
/// declarations keep their mandatory initializer.
fn is_nil_initialized(local: &LocalAssignment) -> bool {
    !local.is_const
        && local.names.len() == 1
        && local
            .exprs
            .as_ref()
            .is_some_and(|exprs| matches!(exprs.items.as_slice(), [value] if is_nil(value)))
}

/// Only a bare `return` is removable. `return nil` returns one value, and
/// `select('#', f())` observes the difference.
fn ends_in_bare_return(block: &Block) -> bool {
    matches!(
        block.last_stmt.as_deref(),
        Some(LastStatement::Return(return_stmt)) if return_stmt.exprs.is_empty()
    )
}

/// `cond and X or X`, which a run of `or`s starts with when its first two
/// operands are that.
fn is_redundant_ternary(outer: &BinaryOp) -> bool {
    let Expression::BinaryOp(inner) = &outer.first else {
        return false;
    };
    let alternative = &outer.links.first().operand;
    let conditions = inner.links.split_last().map_or(&[][..], |(_, before_last)| before_last);
    outer.links.first().op == BinOp::Or
        && inner.links.first().op == BinOp::And
        && is_same_pure_expr(&inner.links.last().operand, alternative)
        && is_pure_expr(&inner.first)
        && conditions.iter().all(|link| is_pure_expr(&link.operand))
        // X must be pure too: a call that returns a falsy value runs twice in
        // the original but once here.
        && is_pure_expr(alternative)
}

impl DeadCodeRemover {
    /// Rewrites the chain in one walk over its arms, in the order Lua tests
    /// them, and walks each arm it keeps once. An arm under a falsy literal
    /// never runs. The first under a truthy literal runs whenever it's
    /// reached, so it becomes the `else` and every arm after it goes.
    fn simplify_if(&mut self, mut if_stmt: Box<IfStatement>) -> Statement {
        let IfStatement { span, condition, block, elseif_clauses, else_clause } = &mut *if_stmt;
        *condition = self.transform_expr(condition.take());
        let mut has_head = match constant_truthiness(condition) {
            Some(true) => return self.keep_only_block(block.take()),
            Some(false) => false,
            None => {
                *block = self.transform_block(block.take());
                true
            }
        };
        let mut has_dropped_arm = !has_head;
        let mut taken_block = None;
        elseif_clauses.retain_mut(|clause| {
            if taken_block.is_some() {
                return false;
            }
            clause.condition = self.transform_expr(clause.condition.take());
            let truthiness = constant_truthiness(&clause.condition);
            has_dropped_arm |= truthiness.is_some();
            match truthiness {
                Some(false) => false,
                Some(true) => {
                    taken_block = Some(clause.block.take());
                    false
                }
                None if has_head => {
                    clause.block = self.transform_block(clause.block.take());
                    true
                }
                // The first arm left takes the dropped head's place.
                None => {
                    has_head = true;
                    *condition = clause.condition.take();
                    *block = self.transform_block(clause.block.take());
                    false
                }
            }
        });
        if let Some(taken) = taken_block {
            *else_clause = Some(ElseClause { span: Span::default(), block: taken });
        }
        if let Some(clause) = else_clause {
            clause.block = self.transform_block(clause.block.take());
        }
        if has_dropped_arm {
            self.has_changed = true;
            *span = Span::default();
        }
        if !has_head {
            return match else_clause.take() {
                Some(clause) => do_block(clause.block),
                // `if false then ... end`, which `is_dead_branch` drops.
                None => Statement::IfStatement(if_stmt),
            };
        }

        if elseif_clauses.is_empty() && else_clause.is_some() {
            // The `not` and its parentheses nest the condition two levels
            // deeper.
            if is_empty_block(block) && fits_at_depth(self.depth, 2, condition) {
                return self.negate_into_else(if_stmt);
            }
            if is_negation(condition) {
                return self.swap_negated_branches(if_stmt);
            }
        }
        Statement::IfStatement(if_stmt)
    }

    fn keep_only_block(&mut self, block: Block) -> Statement {
        self.has_changed = true;
        do_block(self.transform_block(block))
    }

    fn negate_into_else(&mut self, mut if_stmt: Box<IfStatement>) -> Statement {
        self.has_changed = true;
        if_stmt.span = Span::default();
        if_stmt.condition = negate_expr(if_stmt.condition.take());
        if let Some(else_clause) = if_stmt.else_clause.take() {
            if_stmt.block = else_clause.block;
        }
        Statement::IfStatement(if_stmt)
    }

    /// `if not c then A else B end` becomes `if c then B else A end`.
    fn swap_negated_branches(&mut self, mut if_stmt: Box<IfStatement>) -> Statement {
        self.has_changed = true;
        if_stmt.span = Span::default();
        if let Expression::UnaryOp(unop) = if_stmt.condition.take() {
            if_stmt.condition = unop.operand;
        }
        if let Some(else_clause) = &mut if_stmt.else_clause {
            mem::swap(&mut if_stmt.block, &mut else_clause.block);
            else_clause.span = Span::default();
        }
        Statement::IfStatement(if_stmt)
    }

    // Out of line, so an expression nested below holds only the walk's frame,
    // not these temporaries.
    fn fold_redundant_ternary(&mut self, walked: Expression) -> Expression {
        match walked {
            // `cond and X or X` folds to `X` when both branches are identical
            // and the condition is pure.
            Expression::BinaryOp(outer) if is_redundant_ternary(&outer) => {
                self.has_changed = true;
                let BinaryOp { span, links, .. } = *outer;
                let mut rest = links.into_vec();
                let kept = truncate_multi_value(rest.remove(0).operand);
                match OneOrMore::from_vec(rest) {
                    Some(rest_links) => Expression::BinaryOp(Box::new(BinaryOp {
                        span,
                        first: kept,
                        links: rest_links,
                    })),
                    None => kept,
                }
            }
            other @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_)) => other,
        }
    }
}

impl Transform for DeadCodeRemover {
    fn descend(&mut self) {
        self.depth = self.depth.saturating_add(1);
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn transform_block(&mut self, block: Block) -> Block {
        self.descend();
        let mut new_stmts: Vec<Statement> = Vec::with_capacity(block.stmts.len());
        for stmt in block.stmts {
            let transformed = self.transform_stmt(stmt);
            // Unused-local removal first: a dropped/extracted statement
            // never reaches the branch checks below.
            match classify_dead_local(&transformed, &self.live_names, self.version) {
                DeadLocalAction::Keep => {
                    if is_dead_branch(&transformed) {
                        self.has_changed = true;
                    } else {
                        new_stmts.push(transformed);
                    }
                }
                DeadLocalAction::Remove => self.has_changed = true,
                DeadLocalAction::ExtractCalls => {
                    if let Statement::LocalAssignment(local) = transformed {
                        new_stmts.extend(call_stmts(*local));
                    }
                }
            }
        }

        let last_stmt = block.last_stmt.map(|mut last| {
            *last = self.transform_last_stmt(*last);
            last
        });
        self.ascend();

        Block { span: block.span, stmts: new_stmts, last_stmt }
    }

    fn transform_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        self.descend();
        body.block = self.transform_block(body.block);
        self.ascend();
        if ends_in_bare_return(&body.block) {
            self.has_changed = true;
            body.block.last_stmt = None;
        }
        body
    }

    fn transform_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::IfStatement(if_stmt) => self.simplify_if(if_stmt),
            Statement::RepeatLoop(repeat_loop) if runs_once(&repeat_loop) => {
                self.keep_only_block(repeat_loop.block)
            }
            Statement::NumericFor(mut numeric_for) => {
                if has_default_step(&numeric_for) {
                    self.has_changed = true;
                    numeric_for.step = None;
                }
                self.walk_stmt(Statement::NumericFor(numeric_for))
            }
            Statement::LocalAssignment(mut local) => {
                if is_nil_initialized(&local) {
                    self.has_changed = true;
                    local.span = Span::default();
                    local.exprs = None;
                    return Statement::LocalAssignment(local);
                }
                self.walk_stmt(Statement::LocalAssignment(local))
            }
            other @ (Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_)) => self.walk_stmt(other),
        }
    }

    fn transform_expr(&mut self, expr: Expression) -> Expression {
        let walked = self.walk_expr(expr);
        self.fold_redundant_ternary(walked)
    }
}

fn do_block(block: Block) -> Statement {
    Statement::DoBlock(Box::new(DoBlock { span: Span::default(), block }))
}

fn negate_expr(expr: Expression) -> Expression {
    // Only wrap in `not`. Inverting a comparison operator would change which
    // metamethod is called, __lt instead of __le and so on.
    let needs_parens =
        matches!(&expr, Expression::BinaryOp(binary_op) if binds_looser_than_unary(binary_op));
    let operand = if needs_parens { parenthesize(expr) } else { expr };
    Expression::UnaryOp(Box::new(UnaryOp { span: Span::default(), op: UnOp::Not, operand }))
}

enum DeadLocalAction {
    Keep,
    Remove,
    /// Every initializer is pure or a call, so only the calls stay.
    ExtractCalls,
}

fn classify_dead_local(
    stmt: &Statement,
    live_names: &LiveNames,
    version: LuaVersion,
) -> DeadLocalAction {
    match stmt {
        Statement::LocalAssignment(local) => {
            if local
                .names
                .iter()
                .any(|attributed| live_names.contains(identifier_name(&attributed.name)))
            {
                return DeadLocalAction::Keep;
            }
            unused_local_action(local, version)
        }
        Statement::LocalFunction(local_function) => {
            if is_droppable_function(local_function, version)
                && !live_names.contains(identifier_name(&local_function.name))
            {
                DeadLocalAction::Remove
            } else {
                DeadLocalAction::Keep
            }
        }
        Statement::DoBlock(do_block) => {
            if is_empty_block(&do_block.block) {
                DeadLocalAction::Remove
            } else {
                DeadLocalAction::Keep
            }
        }
        Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::WhileLoop(_)
        | Statement::RepeatLoop(_)
        | Statement::IfStatement(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalFunction(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => DeadLocalAction::Keep,
    }
}

/// What goes of `local` when none of its names is live.
///
/// The transform can't make a value less pure, so a `local` this finds
/// droppable before the round is droppable after it.
fn unused_local_action(local: &LocalAssignment, version: LuaVersion) -> DeadLocalAction {
    // Exports, attributed names, and a 5.2+ `_ENV` are never dead.
    if local.is_exported || has_attributed_or_env_name(local, version) {
        return DeadLocalAction::Keep;
    }
    let Some(exprs) = &local.exprs else {
        return DeadLocalAction::Remove;
    };
    if exprs.iter().all(is_pure_expr) {
        DeadLocalAction::Remove
    } else if exprs.iter().all(|expr| is_pure_expr(expr) || is_call(expr)) {
        DeadLocalAction::ExtractCalls
    } else {
        DeadLocalAction::Keep
    }
}

fn is_droppable_function(local_function: &LocalFunction, version: LuaVersion) -> bool {
    !local_function.kind.is_exported()
        && !is_env_binding(identifier_name(&local_function.name), version)
}

fn is_call(expr: &Expression) -> bool {
    matches!(expr, Expression::Suffixed(chain) if chain.is_call())
}

fn call_stmts(local: LocalAssignment) -> impl Iterator<Item = Statement> {
    local.exprs.into_iter().flat_map(Punctuated::into_items).filter_map(|expr| {
        if let Expression::Suffixed(chain) = expr
            && chain.is_call()
        {
            Some(Statement::FunctionCall(chain))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use super::*;
    use crate::fixtures::{parse, run_pass};

    fn apply(source: &str) -> String {
        run_pass(source, LuaVersion::Lua54, |block| remove(block, LuaVersion::Lua54))
    }

    #[test]
    fn removes_unused_local_literal() {
        let result = apply("local unused = 42\nlocal used = 1\nreturn used\n");
        assert!(!result.contains("unused"), "Unused local not removed: {result}");
        assert!(result.contains("used"), "Used local was removed: {result}");
    }

    #[test]
    fn keeps_unused_env_only_where_it_redirects_globals() {
        let source = "local _ENV = {}\nreturn 1\n";
        let kept = apply(source);
        assert!(kept.contains("_ENV"), "5.4 must pin _ENV: {kept}");

        for version in [LuaVersion::Lua51, LuaVersion::Luau] {
            let removed = run_pass(source, version, |block| remove(block, version));
            assert!(
                !removed.contains("_ENV"),
                "{version:?} binds _ENV like any other local: {removed}"
            );
        }
    }

    #[test]
    fn keeps_local_rooting_a_function_declaration() {
        for source in [
            "local M = {}\nfunction M.foo() end\nprint(\"ok\")\n",
            "local M = {}\nfunction M:foo() end\nprint(\"ok\")\n",
            "local M = {}\nfunction M.a.b() end\nprint(\"ok\")\n",
            "local M = {}\ndo function M.foo() end end\nprint(\"ok\")\n",
            "local M\nfunction M() end\nprint(\"ok\")\n",
        ] {
            let result = apply(source);
            assert!(result.contains("local M"), "{source:?} must keep `local M`: {result}");
        }
    }

    #[test]
    fn removes_unused_local_named_like_a_function_declaration_field() {
        let result = apply("local foo = {}\nfunction M.foo() end\n");
        assert!(!result.contains("local foo"), "only the root is a reference: {result}");
    }

    /// One round of the pass, without `remove`'s loop.
    fn apply_one_round(source: &str) -> String {
        run_pass(source, LuaVersion::Lua54, |block| {
            let Some(live_names) = scan(&block, LuaVersion::Lua54) else {
                return block;
            };
            let mut remover = DeadCodeRemover {
                live_names,
                version: LuaVersion::Lua54,
                has_changed: false,
                depth: 0,
            };
            remover.transform_block(block)
        })
    }

    #[test]
    fn removes_a_run_of_locals_each_used_only_by_the_next_in_one_round() {
        let mut forward = String::from("local function f0() end\n");
        for (i, previous) in (1..300u32).zip(0u32..) {
            let _unused = writeln!(forward, "local function f{i}() f{previous}() end");
        }
        let mut backward = String::new();
        for (i, next) in (0..300u32).zip(1u32..) {
            let _unused = writeln!(backward, "local f{i} = function() f{next}() end");
        }
        for source in [forward, backward] {
            assert_eq!(apply_one_round(&source), "", "source: {source}");
        }
    }

    #[test]
    fn keeps_a_local_used_by_a_kept_local() {
        let source = "local a = function() end\nlocal b = function() return a end\nreturn b\n";
        assert_eq!(
            apply_one_round(source),
            "local a=function()end local b=function()return a end return b"
        );
    }

    #[test]
    fn removes_an_unused_local_function_that_calls_itself() {
        assert_eq!(apply_one_round("local function f() return f() end\n"), "");
    }

    #[test]
    fn a_kept_call_keeps_what_it_uses() {
        for (source, expected) in [
            ("local y = 1\nlocal x = function() return y end, g()\n", "g()"),
            ("local y = 1\nlocal x = g(y)\n", "local y=1 g(y)"),
        ] {
            assert_eq!(apply_one_round(source), expected, "source: {source}");
        }
    }

    #[test]
    fn makes_each_rewrite_when_it_is_the_only_one() {
        for (source, expected) in [
            ("local function f() return end return f", "local function f()end return f"),
            ("for i = 1, 2, 1 do f(i) end", "for i=1,2 do f(i)end"),
            ("local x = nil return x", "local x return x"),
            ("repeat f() until true", "do f()end"),
            ("if not a then f() else g() end", "if a then g()else f()end"),
            ("if a then else f() end", "if not a then f()end"),
            ("if a then f() elseif 1 then g() end", "if a then f()else g()end"),
            ("if {} then end", ""),
            ("while nil do f() end", ""),
            ("do end", ""),
            ("local x = f()", "f()"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn skips_a_round_that_would_rewrite_nothing() {
        let block = parse(
            "local a = f() local function g() return a end if a then g() else h() end return g",
            LuaVersion::Lua54,
        );
        assert!(scan(&block, LuaVersion::Lua54).is_none());
    }

    #[test]
    fn removes_unused_local_function() {
        let result = apply("local function unused() return 1 end\nreturn 2\n");
        assert!(!result.contains("unused"), "Unused function not removed: {result}");
    }

    #[test]
    fn removes_if_false() {
        let result = apply("if false then print(1) end\n");
        assert!(!result.contains("print"), "if false branch not removed: {result}");
    }

    #[test]
    fn if_false_promotes_elseif_and_keeps_else() {
        let result = apply("if false then print(1) elseif x then print(2) else print(3) end\n");
        assert!(!result.contains("print(1)"), "dead branch must go: {result}");
        assert!(result.contains("print(2)"), "elseif branch must survive: {result}");
        assert!(result.contains("print(3)"), "else branch must survive: {result}");
        assert!(result.contains("if x"), "elseif must be promoted to if: {result}");
    }

    #[test]
    fn if_false_chained_elseifs_collapse_to_else() {
        let result = apply("if false then print(1) elseif false then print(2) else print(3) end\n");
        assert!(!result.contains("print(1)"), "dead branch must go: {result}");
        assert!(!result.contains("print(2)"), "dead elseif must go: {result}");
        assert!(result.contains("print(3)"), "else branch must survive: {result}");
    }

    #[test]
    fn drops_falsy_arms_anywhere_in_the_chain() {
        for (source, expected) in [
            (
                "if a then f(1) elseif false then f(2) elseif b then f(3) elseif nil then f(4) else f(5) end",
                "if a then f(1)elseif b then f(3)else f(5)end",
            ),
            ("if false then f(1) elseif nil then f(2) elseif a then f(3) end", "if a then f(3)end"),
            ("if false then f(1) elseif nil then f(2) else f(3) end", "do f(3)end"),
            ("if false then f(1) elseif nil then f(2) end", ""),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn turns_the_first_truthy_arm_into_the_else() {
        for (source, expected) in [
            (
                "if a then f(1) elseif 1 then f(2) elseif b then f(3) else f(4) end",
                "if a then f(1)else f(2)end",
            ),
            (
                "if false then f(1) elseif a then f(2) elseif 'x' then f(3) elseif true then f(4) end",
                "if a then f(2)else f(3)end",
            ),
            ("if false then f(1) elseif true then f(2) elseif a then f(3) end", "do f(2)end"),
            ("if true then f(1) elseif a then f(2) else f(3) end", "do f(1)end"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_every_arm_whose_condition_is_not_a_literal() {
        let source = "if a then f(1) elseif b() then f(2) elseif c.d then f(3) end";
        assert_eq!(apply(source), "if a then f(1)elseif b()then f(2)elseif c.d then f(3)end");
    }

    #[test]
    fn collapses_a_long_run_of_falsy_arms() {
        let source = format!(
            "if false then f(){} elseif a then g() end",
            " elseif false then f()".repeat(5_000)
        );
        assert_eq!(apply(&source), "if a then g()end");
    }

    #[test]
    fn unwraps_if_true() {
        let result = apply("if true then print(1) end\n");
        assert!(result.contains("print"), "Body should remain: {result}");
    }

    #[test]
    fn removes_while_false() {
        let result = apply("while false do print(1) end\n");
        assert!(!result.contains("print"), "while false body not removed: {result}");
    }

    #[test]
    fn drops_nil_initializer() {
        let result = apply("local x = nil\nreturn x\n");
        assert!(
            !result.contains("= nil") && !result.contains("=nil"),
            "Should remove = nil: {result}"
        );
        assert!(result.contains("local"), "Should keep declaration: {result}");
    }

    #[test]
    fn keeps_global_self_assignment() {
        // `x = x` on a global fires __index and __newindex under a metatabled
        // environment. Removal is only sound for proven locals, and this pass
        // has no binding info.
        let result = apply("x = x\nreturn 1\n");
        assert!(
            result.contains("x=x") || result.contains("x = x"),
            "global self-assignment must be preserved: {result}"
        );
    }

    #[test]
    fn repeat_until_true_without_break_becomes_do_block() {
        let result = apply("repeat print(1) until true\n");
        assert!(result.contains("do"), "repeat->do rewrite expected: {result}");
        assert!(!result.contains("repeat"), "repeat should be gone: {result}");
    }

    #[test]
    fn repeat_until_true_with_break_is_kept() {
        // `break` binds to the repeat; a do-block would rebind it to an
        // outer loop (or make it invalid).
        let nested = apply("while f() do repeat if g() then break end until true end\n");
        assert!(nested.contains("repeat"), "repeat with break must be kept: {nested}");
        let bare = apply("repeat break until true\n");
        assert!(bare.contains("repeat"), "repeat with break must be kept: {bare}");
    }

    #[test]
    fn negated_empty_then_condition_keeps_its_grouping() {
        for condition in ["a == b", "a and b", "a or b"] {
            let result = apply(&format!("if {condition} then else print(1) end\n"));
            assert!(
                !result.contains("if not a"),
                "`not` must apply to all of `{condition}`, or it regroups as `(not a) ...`: {result}"
            );
        }
    }

    #[test]
    fn keeps_unused_local_reading_a_global() {
        let result = apply("local unused = g\nreturn 1\n");
        assert!(result.contains('g'), "a global read can fire __index, so it must stay: {result}");
    }

    #[test]
    fn keeps_unused_local_whose_literal_operation_raises() {
        for initializer in
            ["1 // 0", "1 % 0", "nil + 1", "#nil", "\"a\" .. nil", "1 < \"a\"", "-\"a\""]
        {
            let result = apply(&format!("local unused = {initializer}\nreturn 1\n"));
            assert!(
                result.contains("local unused"),
                "`{initializer}` raises at runtime, so it must stay: {result}"
            );
        }
    }

    #[test]
    fn keeps_unused_table_whose_constructor_raises() {
        for k in ["nil", "0/0"] {
            let result = apply(&format!("local unused = {{[{k}] = 1}}\nreturn 1\n"));
            assert!(
                result.contains("local unused"),
                "a table key of `{k}` raises at runtime, so it must stay: {result}"
            );
        }
    }

    #[test]
    fn keeps_one_value_when_folding_a_vararg_ternary() {
        assert_eq!(apply("return true and ... or ...\n"), "return(...)");
    }

    #[test]
    fn folds_a_ternary_whose_branches_are_written_alike() {
        for (source, expected) in [
            ("return ... and 1 or 1\n", "return 1"),
            ("return ... and -2 ^ 3 or -2 ^ 3\n", "return-2^3"),
            ("return ... and {1, x = \"a\"} or {1, x = \"a\"}\n", "return{1,x=\"a\"}"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_a_ternary_whose_branches_differ() {
        for source in [
            "return ... and 1 or 2\n",
            "return ... and 1 or 1.0\n",
            "return ... and {1} or {2}\n",
            "return ... and function() end or function() end\n",
        ] {
            assert!(apply(source).contains("or"), "source: {source}");
        }
    }
}
