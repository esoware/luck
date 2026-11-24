//! A function's control-flow graph, which every rule that asks for one
//! shares. Each graph covers one block and stops at nested function bodies,
//! whose flow is their own.

use core::cell::OnceCell;
use core::mem;
use core::num::Saturating;

use luck_ast::{
    BinaryOp, Block, FunctionBody, GenericFor, GetSpan as _, IfExpression, IfStatement, Name,
    NumericFor, RepeatLoop, Statement, Type, Visit, WhileLoop,
};
use luck_semantic::LabelId;
use luck_token::{BinOp, Span};
use luck_value::ToBoolean as _;
use rustc_hash::FxHashMap;

use crate::call::is_global_call;
use crate::context::ContextHost;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Off the block's end, which in a loop body starts the next iteration,
    /// as `continue` does.
    End,
    /// `break`, or a `goto` to a label outside the block.
    Break,
    ReturnValue,
    ReturnEmpty,
    /// A call to the host's `error`.
    Error,
}

impl Exit {
    const fn node(self) -> usize {
        match self {
            Self::End => 0,
            Self::Break => 1,
            Self::ReturnValue => 2,
            Self::ReturnEmpty => 3,
            Self::Error => 4,
        }
    }
}

/// How many nodes the [`Exit`]s take, at the start of every graph.
const EXIT_NODE_COUNT: usize = 5;

/// A node is one piece of code that runs as a unit: a run of simple
/// statements, a condition, or a loop header. No node has more than two
/// successors.
///
/// A run takes every simple statement one block holds in a row, so a walk
/// over a long straight stretch costs one node, not one per statement.
#[derive(Debug)]
pub(crate) struct FlowGraph {
    successors: Vec<[Option<usize>; 2]>,
    /// Where each node's code starts, or 0 for a node without code.
    code_starts: Vec<u32>,
    /// The span and node of each piece of code, in source order and disjoint.
    code: Vec<(Span, usize)>,
    locals: Vec<LocalDeclaration>,
    is_reachable: Vec<bool>,
    /// Each loop body, in source order.
    loops: Vec<LoopBody>,
    /// What each loop's body reaches, in the order of `loops`, built on
    /// first use.
    loop_exits: OnceCell<Box<[Exits]>>,
}

#[derive(Debug, Clone, Copy)]
struct LoopBody {
    /// Where the loop statement starts.
    start: u32,
    /// The body's first node, which nothing outside the body but the loop
    /// enters by. The nodes from it to `end` are the body's.
    entry: usize,
    end: usize,
    continue_target: usize,
}

/// The [`Exit`]s a block reaches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Exits(u8);

impl Exits {
    pub(crate) const fn reaches(self, exit: Exit) -> bool {
        self.0 & (1 << exit.node()) != 0
    }

    const fn insert(&mut self, exit: Exit) {
        self.0 |= 1 << exit.node();
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalDeclaration {
    pub name_span: Span,
    pub node: usize,
    /// Where the name's scope ends. Code past this can't reference it, and
    /// Lua can only re-enter a scope through the `local` statement itself.
    pub scope_end: u32,
}

impl FlowGraph {
    /// The graph of `block`, which a function body's block is in every
    /// graph a rule reads.
    pub(crate) fn of_block<'ast>(block: &'ast Block<'ast>, host: &ContextHost<'ast>) -> Self {
        let mut builder = GraphBuilder {
            graph: Self {
                successors: vec![[None; 2]; EXIT_NODE_COUNT],
                code_starts: vec![0; EXIT_NODE_COUNT],
                code: Vec::new(),
                locals: Vec::new(),
                is_reachable: Vec::new(),
                loops: Vec::new(),
                loop_exits: OnceCell::new(),
            },
            host,
            cursor: None,
            run: None,
            scope_end: 0,
            break_target: Exit::Break.node(),
            continue_target: Exit::End.node(),
            label_nodes: FxHashMap::default(),
        };
        let entry = builder.new_node();
        builder.cursor = Some(entry);
        builder.block(block);
        builder.close_into(Exit::End.node());

        let mut graph = builder.graph;
        let mut is_reachable = vec![false; graph.node_count()];
        let mut stack = vec![entry];
        while let Some(node) = stack.pop() {
            if let Some(mark) = is_reachable.get_mut(node)
                && !mem::replace(mark, true)
            {
                stack.extend(graph.successors(node));
            }
        }
        graph.is_reachable = is_reachable;
        graph
    }

    pub(crate) fn reaches(&self, exit: Exit) -> bool {
        self.is_reachable.get(exit.node()).copied().unwrap_or_default()
    }

    #[cfg(test)]
    fn exits(&self) -> Exits {
        let mut exits = Exits::default();
        for exit in [Exit::End, Exit::Break, Exit::ReturnValue, Exit::ReturnEmpty, Exit::Error] {
            if self.reaches(exit) {
                exits.insert(exit);
            }
        }
        exits
    }

    /// The exits the body of the loop that starts at `loop_start` reaches,
    /// as the graph of that body alone has them, where a jump out of the
    /// body is a `Break`.
    pub(crate) fn loop_exits(&self, loop_start: u32) -> Option<Exits> {
        let index = self.loops.binary_search_by_key(&loop_start, |body| body.start).ok()?;
        self.loop_exits.get_or_init(|| self.every_loop_s_exits()).get(index).copied()
    }

    /// Inner bodies go first, so an outer one steps over each in one move to
    /// what it reaches past its end, and the whole costs one pass.
    fn every_loop_s_exits(&self) -> Box<[Exits]> {
        let mut reached: Vec<Vec<usize>> = vec![Vec::new(); self.loops.len()];
        let mut visited_in = vec![usize::MAX; self.node_count()];
        let mut stack = Vec::new();
        for (index, body) in self.loops.iter().enumerate().rev() {
            let mut past_end = Vec::new();
            stack.push(body.entry);
            while let Some(node) = stack.pop() {
                if visited_in.get_mut(node).is_none_or(|visit| mem::replace(visit, index) == index)
                {
                    continue;
                }
                if !(body.entry..body.end).contains(&node) {
                    past_end.push(node);
                } else if node != body.entry
                    && let Ok(inner) = self.loops.binary_search_by_key(&node, |inner| inner.entry)
                {
                    stack.extend(reached.get(inner).into_iter().flatten());
                } else {
                    stack.extend(self.successors(node));
                }
            }
            if let Some(slot) = reached.get_mut(index) {
                *slot = past_end;
            }
        }
        self.loops
            .iter()
            .zip(reached)
            .map(|(body, past_end)| {
                let mut exits = Exits::default();
                for node in past_end {
                    let exit = if node == body.continue_target {
                        Exit::End
                    } else {
                        // Else the loop's `break`, or a jump out of its body.
                        [Exit::ReturnValue, Exit::ReturnEmpty, Exit::Error]
                            .into_iter()
                            .find(|exit| exit.node() == node)
                            .unwrap_or(Exit::Break)
                    };
                    exits.insert(exit);
                }
                exits
            })
            .collect()
    }

    /// Whether the code at `offset`, or else the first code after it, can
    /// run. True past the last code.
    pub(crate) fn can_run(&self, offset: u32) -> bool {
        self.node_at(offset)
            .is_none_or(|node| self.is_reachable.get(node).copied().unwrap_or_default())
    }

    /// The node of the code at `offset`, or else of the first code after it.
    pub(crate) fn node_at(&self, offset: u32) -> Option<usize> {
        let index = self.code.partition_point(|&(span, _)| span.end <= offset);
        self.code.get(index).map(|&(_, node)| node)
    }

    pub(crate) const fn node_count(&self) -> usize {
        self.successors.len()
    }

    pub(crate) fn successors(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        self.successors.get(node).into_iter().flatten().copied().flatten()
    }

    pub(crate) fn locals(&self) -> &[LocalDeclaration] {
        &self.locals
    }

    pub(crate) fn starts_at_or_after(&self, node: usize, offset: u32) -> bool {
        self.code_starts.get(node).is_some_and(|start| *start >= offset)
    }
}

struct GraphBuilder<'host, 'ast> {
    graph: FlowGraph,
    host: &'host ContextHost<'ast>,
    /// The node the next code runs after, or `None` right after a jump.
    cursor: Option<usize>,
    /// The run of simple statements the cursor ends, which the next one
    /// joins.
    run: Option<usize>,
    scope_end: u32,
    break_target: usize,
    continue_target: usize,
    /// The node of each label of a block the builder has entered, which
    /// the `goto`s semantic analysis resolved to it jump to.
    label_nodes: FxHashMap<LabelId, usize>,
}

impl<'ast> GraphBuilder<'_, 'ast> {
    fn new_node(&mut self) -> usize {
        self.graph.successors.push([None; 2]);
        self.graph.code_starts.push(0);
        self.graph.node_count().saturating_sub(1)
    }

    fn connect(&mut self, from: usize, to: usize) {
        if let Some(successors) = self.graph.successors.get_mut(from)
            && let Some(free_edge) = successors.iter_mut().find(|edge| edge.is_none())
        {
            *free_edge = Some(to);
        }
    }

    fn advance_to(&mut self, node: usize) {
        if let Some(cursor) = self.cursor {
            self.connect(cursor, node);
        }
        self.cursor = Some(node);
    }

    fn code(&mut self, span: Span) -> usize {
        let node = self.new_node();
        if let Some(code_start) = self.graph.code_starts.get_mut(node) {
            *code_start = span.start;
        }
        self.graph.code.push((span, node));
        self.advance_to(node);
        node
    }

    /// A simple statement, which runs straight on from the one before it in
    /// its block.
    fn simple(&mut self, span: Span) -> usize {
        if let Some(run) = self.run
            && self.cursor == Some(run)
        {
            self.graph.code.push((span, run));
            return run;
        }
        let node = self.code(span);
        self.run = Some(node);
        node
    }

    fn jump(&mut self, span: Span, target: usize) {
        let node = self.code(span);
        self.connect(node, target);
        self.cursor = None;
    }

    fn close_into(&mut self, target: usize) {
        if let Some(cursor) = self.cursor.take() {
            self.connect(cursor, target);
        }
    }

    fn block(&mut self, block: &'ast Block<'ast>) {
        self.scoped_block(block, block.span.end);
    }

    fn scoped_block(&mut self, block: &'ast Block<'ast>, scope_end: u32) {
        self.run = None;
        let outer_scope_end = mem::replace(&mut self.scope_end, scope_end);
        // A label is visible in its whole block, `goto`s before it included,
        // so its node comes before the code of every block inside it.
        for stmt in &block.stmts {
            if let Statement::Label(label) = stmt
                && let Some(id) = self.host.scoping().label_of(&label.name)
            {
                let node = self.new_node();
                self.label_nodes.insert(id, node);
            }
        }
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
        self.scope_end = outer_scope_end;
        self.run = None;
    }

    /// The node of the label `name` names, or reaches as a `goto`'s.
    fn label_node(&self, name: &Name<'_>) -> Option<usize> {
        let id = self.host.scoping().label_of(name)?;
        self.label_nodes.get(&id).copied()
    }

    fn stmt(&mut self, stmt: &'ast Statement<'ast>) {
        match stmt {
            Statement::FunctionCall(chain) => {
                let node = self.simple(chain.span);
                if chain
                    .last_call()
                    .is_some_and(|call| is_global_call(call, "error", self.host.semantic))
                {
                    self.connect(node, Exit::Error.node());
                    self.cursor = None;
                }
            }
            Statement::LocalAssignment(local) => {
                let node = self.simple(local.span);
                let scope_end = self.scope_end;
                self.graph.locals.extend(local.names.iter().map(|attributed| LocalDeclaration {
                    name_span: attributed.name.span,
                    node,
                    scope_end,
                }));
            }
            Statement::DoBlock(node) => self.block(&node.block),
            Statement::WhileLoop(node) => self.while_loop(node),
            Statement::RepeatLoop(node) => self.repeat_loop(node),
            Statement::IfStatement(node) => self.if_stmt(node),
            Statement::NumericFor(node) => {
                self.for_loop(Span::new(node.span.start, node.block.span.start), &node.block);
            }
            Statement::GenericFor(node) => {
                self.for_loop(Span::new(node.span.start, node.block.span.start), &node.block);
            }
            // Lua 5.2+
            Statement::Goto(node) => {
                let target = self.label_node(&node.name);
                self.jump(node.span, target.unwrap_or_else(|| Exit::Break.node()));
            }
            // Lua 5.2+
            Statement::Label(node) => {
                if let Some(target) = self.label_node(&node.name) {
                    self.graph.code.push((node.span, target));
                    self.advance_to(target);
                }
            }
            Statement::Return(node) => {
                let exit =
                    if node.exprs.is_empty() { Exit::ReturnEmpty } else { Exit::ReturnValue };
                self.jump(node.span, exit.node());
            }
            Statement::Break(span) => self.jump(*span, self.break_target),
            // Luau
            Statement::Continue(span) => self.jump(*span, self.continue_target),
            Statement::Assignment(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {
                self.simple(stmt.span());
            }
        }
    }

    fn loop_body(
        &mut self,
        loop_start: u32,
        block: &'ast Block<'ast>,
        scope_end: u32,
        break_target: usize,
        continue_target: usize,
    ) -> usize {
        let entry = self.new_node();
        self.advance_to(entry);
        let index = self.graph.loops.len();
        self.graph.loops.push(LoopBody { start: loop_start, entry, end: entry, continue_target });
        let outer_break_target = mem::replace(&mut self.break_target, break_target);
        let outer_continue_target = mem::replace(&mut self.continue_target, continue_target);
        self.scoped_block(block, scope_end);
        self.close_into(continue_target);
        self.break_target = outer_break_target;
        self.continue_target = outer_continue_target;
        let end = self.graph.node_count();
        if let Some(body) = self.graph.loops.get_mut(index) {
            body.end = end;
        }
        entry
    }

    fn while_loop(&mut self, node: &'ast WhileLoop<'ast>) {
        let head = self.code(Span::new(node.span.start, node.block.span.start));
        let after = self.new_node();
        let truth = node.condition.to_boolean(self.host);
        if truth == Some(false) {
            self.cursor = None;
        }
        self.loop_body(node.span.start, &node.block, node.block.span.end, after, head);
        if truth != Some(true) {
            self.connect(head, after);
        }
        self.cursor = Some(after);
    }

    fn repeat_loop(&mut self, node: &'ast RepeatLoop<'ast>) {
        let condition = self.new_node();
        let after = self.new_node();
        // A repeat's condition still sees its body's locals.
        let scope_end = node.condition.span().end;
        let body = self.loop_body(node.span.start, &node.block, scope_end, after, condition);
        self.graph.code.push((node.condition.span(), condition));
        let truth = node.condition.to_boolean(self.host);
        if truth != Some(true) {
            self.connect(condition, body);
        }
        if truth != Some(false) {
            self.connect(condition, after);
        }
        self.cursor = Some(after);
    }

    fn for_loop(&mut self, header: Span, body: &'ast Block<'ast>) {
        self.code(header);
        let head = self.new_node();
        self.advance_to(head);
        let after = self.new_node();
        self.loop_body(header.start, body, body.span.end, after, head);
        // The iterator can run out before the first iteration.
        self.connect(head, after);
        self.cursor = Some(after);
    }

    fn if_stmt(&mut self, node: &'ast IfStatement<'ast>) {
        let mut test = self.code(Span::new(node.span.start, node.block.span.start));
        let join = self.new_node();
        self.block(&node.block);
        self.close_into(join);
        for clause in &node.elseif_clauses {
            self.cursor = Some(test);
            test = self.code(Span::new(clause.span.start, clause.block.span.start));
            self.block(&clause.block);
            self.close_into(join);
        }
        self.cursor = Some(test);
        if let Some(else_clause) = &node.else_clause {
            self.block(&else_clause.block);
        }
        self.close_into(join);
        self.cursor = Some(join);
    }
}

pub(crate) fn decision_points(block: &Block<'_>) -> u32 {
    let mut points = DecisionPoints::default();
    points.visit_block(block);
    points.count.0
}

/// One per `if`, `elseif`, and loop, one per `and` or `or`, and one per `if`
/// or `elseif` of a Luau if-expression.
#[derive(Default)]
struct DecisionPoints {
    count: Saturating<u32>,
}

impl DecisionPoints {
    fn count_clauses(&mut self, clause_count: usize) {
        self.count += u32::try_from(clause_count.saturating_add(1)).unwrap_or(u32::MAX);
    }
}

impl<'ast> Visit<'ast> for DecisionPoints {
    fn visit_if_stmt(&mut self, if_stmt: &'ast IfStatement<'ast>) {
        self.count_clauses(if_stmt.elseif_clauses.len());
        self.walk_if_stmt(if_stmt);
    }

    fn visit_while_loop(&mut self, while_loop: &'ast WhileLoop<'ast>) {
        self.count += 1;
        self.walk_while_loop(while_loop);
    }

    fn visit_repeat_loop(&mut self, repeat_loop: &'ast RepeatLoop<'ast>) {
        self.count += 1;
        self.walk_repeat_loop(repeat_loop);
    }

    fn visit_numeric_for(&mut self, numeric_for: &'ast NumericFor<'ast>) {
        self.count += 1;
        self.walk_numeric_for(numeric_for);
    }

    fn visit_generic_for(&mut self, generic_for: &'ast GenericFor<'ast>) {
        self.count += 1;
        self.walk_generic_for(generic_for);
    }

    fn visit_binary_op(&mut self, binary_op: &'ast BinaryOp<'ast>) {
        if matches!(binary_op.links.first().op, BinOp::And | BinOp::Or) {
            self.count += u32::try_from(binary_op.links.len()).unwrap_or(u32::MAX);
        }
        self.walk_binary_op(binary_op);
    }

    // Luau
    fn visit_if_expr(&mut self, if_expr: &'ast IfExpression<'ast>) {
        self.count_clauses(if_expr.elseif_clauses.len());
        self.walk_if_expr(if_expr);
    }

    fn visit_function_body(&mut self, _function_body: &'ast FunctionBody<'ast>) {}

    fn visit_type(&mut self, _ty: &'ast Type<'ast>) {}
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use luck_allocator::Allocator;
    use luck_ast::Chunk;
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::with_host;

    fn parse_chunk<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
        version: LuaVersion,
    ) -> &'ast Chunk<'ast> {
        let parse = luck_parser::parse(allocator, source.as_bytes(), version);
        assert!(parse.errors.is_empty(), "parser errors for {source:?}: {:?}", parse.errors);
        allocator.alloc(parse.chunk)
    }

    fn flow_of(source: &str, version: LuaVersion) -> FlowGraph {
        with_host(source, version.into(), |host| FlowGraph::of_block(&host.chunk.block, host))
    }

    fn flow(source: &str) -> FlowGraph {
        flow_of(source, LuaVersion::Lua54)
    }

    fn points(source: &str) -> u32 {
        decision_points(&parse_chunk(&Allocator::new(), source, LuaVersion::Lua54).block)
    }

    #[test]
    fn a_straight_run_of_statements_is_one_node() {
        let one = flow("local a").node_count();
        let mut run = String::from("local a, b");
        for i in 0..100u32 {
            let _unused = write!(
                run,
                "
a = {i}
b = f(a)
print(a, b)"
            );
        }
        assert_eq!(flow(&run).node_count(), one);
        let block_between = "\
local a
do b() end
c()";
        assert_eq!(flow(block_between).node_count(), one + 2);
    }

    #[test]
    fn empty_block_falls_through() {
        let graph = flow("");
        assert!(graph.reaches(Exit::End));
        assert!(!graph.reaches(Exit::ReturnValue));
    }

    #[test]
    fn return_ends_the_block() {
        let graph = flow("return 1");
        assert!(graph.reaches(Exit::ReturnValue));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn bare_return_has_no_value() {
        let graph = flow("return");
        assert!(graph.reaches(Exit::ReturnEmpty));
        assert!(!graph.reaches(Exit::ReturnValue));
    }

    #[test]
    fn if_without_else_falls_through() {
        let graph = flow(
            "\
local c = 1
if c then return 1 end",
        );
        assert!(graph.reaches(Exit::End));
        assert!(graph.reaches(Exit::ReturnValue));
    }

    #[test]
    fn if_else_both_returning_never_fall_through() {
        let graph = flow(
            "\
local c = 1
if c then return 1 else return 2 end",
        );
        assert!(graph.reaches(Exit::ReturnValue));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn if_elseif_else_all_returning_never_fall_through() {
        let graph = flow(
            "\
local c = 1
local d = 1
if c then return 1 elseif d then return 2 else return 3 end",
        );
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn while_true_without_break_never_falls_through() {
        let graph = flow("while true do print(1) end");
        assert!(!graph.reaches(Exit::End));
        assert!(!graph.reaches(Exit::ReturnValue));
    }

    #[test]
    fn while_true_with_return_only_returns() {
        let graph = flow("while true do return 1 end");
        assert!(graph.reaches(Exit::ReturnValue));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn while_true_with_conditional_break_falls_through() {
        let graph = flow(
            "\
local c = 1
while true do if c then break end end",
        );
        assert!(graph.reaches(Exit::End));
        assert!(!graph.reaches(Exit::Break), "the `break` belongs to the loop");
    }

    #[test]
    fn while_with_truthy_literal_condition_never_falls_through() {
        for condition in ["1", "'s'", "{}", "(true)", "x or true", "1 < 2"] {
            let graph = flow(&format!("while {condition} do print(1) end"));
            assert!(!graph.reaches(Exit::End), "{condition}");
        }
    }

    #[test]
    fn while_with_falsy_literal_condition_skips_its_body() {
        let graph = flow("while nil do return 1 end");
        assert!(graph.reaches(Exit::End));
        assert!(!graph.reaches(Exit::ReturnValue));
    }

    #[test]
    fn repeat_until_true_runs_once() {
        let graph = flow("repeat return 1 until true");
        assert!(graph.reaches(Exit::ReturnValue));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn repeat_until_false_is_infinite() {
        assert!(!flow("repeat print(1) until false").reaches(Exit::End));
    }

    #[test]
    fn numeric_for_may_not_run() {
        let graph = flow("for i = 1, 10 do return 1 end");
        assert!(graph.reaches(Exit::End));
        assert!(graph.reaches(Exit::ReturnValue));
    }

    #[test]
    fn do_block_return_propagates() {
        let graph = flow("do return end");
        assert!(graph.reaches(Exit::ReturnEmpty));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn error_call_ends_the_block() {
        let graph = flow("error(\"x\")");
        assert!(graph.reaches(Exit::Error));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn shadowed_error_is_an_ordinary_call() {
        let graph = flow(
            "\
local function error() end
error(\"x\")",
        );
        assert!(!graph.reaches(Exit::Error));
        assert!(graph.reaches(Exit::End));
    }

    #[test]
    fn break_in_inner_loop_is_consumed() {
        let graph = flow(
            "\
local c = 1
while c do if c then break end end",
        );
        assert!(graph.reaches(Exit::End));
        assert!(!graph.reaches(Exit::Break));
    }

    #[test]
    fn continue_runs_the_until_condition() {
        let graph = flow_of(
            "\
repeat continue until true
return 2",
            LuaVersion::Luau,
        );
        assert!(graph.reaches(Exit::ReturnValue));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn continue_of_the_analyzed_block_ends_it() {
        let reaches_end =
            with_host("for _ = 1, 3 do continue end", LuaVersion::Luau.into(), |host| {
                let Some(Statement::NumericFor(numeric_for)) = host.chunk.block.stmts.first()
                else {
                    panic!("the statement must be a numeric for");
                };
                FlowGraph::of_block(&numeric_for.block, host).reaches(Exit::End)
            });
        assert!(reaches_end);
    }

    #[test]
    fn goto_jumps_over_the_code_before_its_label() {
        let graph = flow(
            "\
goto skip
do return end
::skip::
print(1)",
        );
        assert!(!graph.reaches(Exit::ReturnEmpty));
        assert!(graph.reaches(Exit::End));
    }

    #[test]
    fn backward_goto_loops() {
        let graph = flow(
            "\
::top::
print(1)
goto top",
        );
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn goto_to_a_label_outside_the_block_leaves_it() {
        let source = "\
while true do goto done end
::done::";
        let graph = with_host(source, LuaVersion::Lua54.into(), |host| {
            let Some(Statement::WhileLoop(while_loop)) = host.chunk.block.stmts.first() else {
                panic!("the statement must be a while loop");
            };
            FlowGraph::of_block(&while_loop.block, host)
        });
        assert!(graph.reaches(Exit::Break));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn goto_takes_the_innermost_label_of_its_name() {
        // The outer `::a::` would loop forever.
        let graph = flow(
            "\
::a::
do
  goto a
  do return end
  ::a::
end",
        );
        assert!(graph.reaches(Exit::End));
        assert!(!graph.reaches(Exit::ReturnEmpty));
    }

    #[test]
    fn label_goes_out_of_sight_with_its_block() {
        let graph = flow(
            "\
do ::a:: end
goto a",
        );
        assert!(graph.reaches(Exit::Break));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn gotos_find_their_labels_among_many() {
        let mut source = String::new();
        for i in 0..2_000u32 {
            let _unused = writeln!(
                source,
                "\
goto l{i}
do return end
::l{i}::"
            );
        }
        let graph = flow(&source);
        assert!(graph.reaches(Exit::End));
        assert!(!graph.reaches(Exit::ReturnEmpty));
    }

    /// The loop bodies a block holds outside its functions, by where each
    /// loop starts.
    #[derive(Default)]
    struct LoopBodies<'ast>(Vec<(u32, &'ast Block<'ast>)>);

    impl<'ast> Visit<'ast> for LoopBodies<'ast> {
        fn visit_while_loop(&mut self, while_loop: &'ast WhileLoop<'ast>) {
            self.0.push((while_loop.span.start, &while_loop.block));
            self.walk_while_loop(while_loop);
        }

        fn visit_repeat_loop(&mut self, repeat_loop: &'ast RepeatLoop<'ast>) {
            self.0.push((repeat_loop.span.start, &repeat_loop.block));
            self.walk_repeat_loop(repeat_loop);
        }

        fn visit_numeric_for(&mut self, numeric_for: &'ast NumericFor<'ast>) {
            self.0.push((numeric_for.span.start, &numeric_for.block));
            self.walk_numeric_for(numeric_for);
        }

        fn visit_generic_for(&mut self, generic_for: &'ast GenericFor<'ast>) {
            self.0.push((generic_for.span.start, &generic_for.block));
            self.walk_generic_for(generic_for);
        }

        fn visit_function_body(&mut self, _function_body: &'ast FunctionBody<'ast>) {}
    }

    #[test]
    fn each_loop_s_exits_are_those_of_a_graph_of_its_body_alone() {
        let sources = [
            ("for i = 1, 2 do while true do goto out end ::out:: end", LuaVersion::Lua54),
            ("::a:: for i = 1, 2 do for j = 1, 2 do goto a end end", LuaVersion::Lua54),
            (
                "for i = 1, 2 do ::top:: while i do if i then goto top end return end end",
                LuaVersion::Lua54,
            ),
            (
                "while x do repeat if y then break end goto done until true end ::done::",
                LuaVersion::Lua54,
            ),
            ("while false do for i = 1, 2 do break end return end", LuaVersion::Lua54),
            ("for i = 1, 2 do while true do end repeat until false end", LuaVersion::Lua54),
            (
                "for i = 1, 2 do local f = function() while x do error() end end end",
                LuaVersion::Lua54,
            ),
            ("for i = 1, 2 do repeat if x then continue end return until x end", LuaVersion::Luau),
            (
                "while x do for _ in t do if x then break end return end continue end",
                LuaVersion::Luau,
            ),
        ];
        for (source, version) in sources {
            with_host(source, version.into(), |host| {
                let graph = FlowGraph::of_block(&host.chunk.block, host);
                let mut bodies = LoopBodies::default();
                bodies.visit_block(&host.chunk.block);
                assert!(!bodies.0.is_empty(), "{source}");
                for (start, body) in bodies.0 {
                    let alone = FlowGraph::of_block(body, host).exits();
                    assert_eq!(
                        graph.loop_exits(start),
                        Some(alone),
                        "the loop at {start} in {source}"
                    );
                }
            });
        }
    }

    #[test]
    fn can_run_is_false_after_a_jump_until_a_reached_label() {
        let source = "\
goto a
print(1)
::a::
print(2)";
        let graph = flow(source);
        let offset_of = |text: &str| u32::try_from(source.find(text).expect("in source")).unwrap();
        assert!(!graph.can_run(offset_of("print(1)")));
        assert!(graph.can_run(offset_of("print(2)")));
    }

    #[test]
    fn decision_points_empty_block() {
        // The rule adds McCabe's baseline of 1; the count here is 0.
        assert_eq!(points(""), 0);
    }

    #[test]
    fn decision_points_single_if() {
        assert_eq!(
            points(
                "\
local c = 1
if c then end"
            ),
            1
        );
    }

    #[test]
    fn decision_points_if_with_and() {
        // +1 for the if, +1 for the `and`.
        assert_eq!(
            points(
                "\
local a = 1
local b = 1
if a and b then end"
            ),
            2
        );
    }

    #[test]
    fn decision_points_if_elseif() {
        // +1 for the if, +1 for the elseif.
        assert_eq!(
            points(
                "\
local a = 1
local b = 1
if a then elseif b then end"
            ),
            2
        );
    }

    #[test]
    fn decision_points_each_loop_kind() {
        assert_eq!(points("while a do end"), 1);
        assert_eq!(points("repeat until a"), 1);
        assert_eq!(points("for i = 1, 2 do end"), 1);
        assert_eq!(points("for _ in f do end"), 1);
    }

    #[test]
    fn decision_points_in_table_call_argument() {
        assert_eq!(points("f{a and b}"), 1);
        assert_eq!(points("f({a and b})"), 1);
    }

    #[test]
    fn decision_points_in_assignment_target() {
        assert_eq!(points("t[a or b] = 1"), 1);
    }

    #[test]
    fn decision_points_in_return() {
        assert_eq!(points("return a and b"), 1);
    }

    #[test]
    fn decision_points_in_return_after_a_terminating_stmt() {
        assert_eq!(points("do return end return a and b"), 1);
    }

    #[test]
    fn decision_points_skip_nested_functions() {
        assert_eq!(points("local f = function() if a then end end"), 0);
        assert_eq!(points("local function f() if a then end end"), 0);
    }

    #[test]
    fn decision_points_nested_elseif_chain() {
        // +1 for the if, +1 per elseif (two of them) = 3.
        assert_eq!(
            points(
                "\
local a = 1
local b = 1
local c = 1
if a then elseif b then elseif c then end"
            ),
            3
        );
    }
}
