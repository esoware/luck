//! Each graph covers one block and stops at nested function bodies, whose
//! flow is their own.

use core::mem;
use core::num::Saturating;

use luck_ast::{
    Block, Expression, FunctionBody, IfStatement, LastStatement, RepeatLoop, Statement, Type,
    Visitor, WhileLoop, unwrap_parentheses,
};
use luck_semantic::SemanticAnalysis;
use luck_token::{BinOp, Span, Token};
use rustc_hash::FxHashMap;

use crate::call::is_global_call;

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
    pub(crate) fn of_block(block: &Block, semantic: &SemanticAnalysis) -> Self {
        let mut builder = GraphBuilder {
            graph: Self {
                successors: vec![[None; 2]; EXIT_NODE_COUNT],
                code_starts: vec![0; EXIT_NODE_COUNT],
                code: Vec::new(),
                locals: Vec::new(),
                is_reachable: Vec::new(),
            },
            semantic,
            cursor: None,
            run: None,
            scope_end: 0,
            break_target: Exit::Break.node(),
            continue_target: Exit::End.node(),
            labels: FxHashMap::default(),
            label_names: Vec::new(),
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

struct GraphBuilder<'ast, 'analysis> {
    graph: FlowGraph,
    semantic: &'analysis SemanticAnalysis,
    /// The node the next code runs after, or `None` right after a jump.
    cursor: Option<usize>,
    /// The run of simple statements the cursor ends, which the next one
    /// joins.
    run: Option<usize>,
    scope_end: u32,
    break_target: usize,
    continue_target: usize,
    /// The node of each label visible here, by name, innermost last.
    labels: FxHashMap<&'ast str, Vec<usize>>,
    /// The name of each label in `labels`, in the order they became visible.
    label_names: Vec<&'ast str>,
}

impl<'ast> GraphBuilder<'ast, '_> {
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

    fn block(&mut self, block: &'ast Block) {
        self.scoped_block(block, block.span.end);
    }

    fn scoped_block(&mut self, block: &'ast Block, scope_end: u32) {
        self.run = None;
        let outer_scope_end = mem::replace(&mut self.scope_end, scope_end);
        let outer_label_count = self.label_names.len();
        // A label is visible in its whole block, `goto`s before it included.
        for stmt in &block.stmts {
            if let Statement::Label(label) = stmt
                && let Some(name) = label.name.identifier()
            {
                let node = self.new_node();
                self.labels.entry(name).or_default().push(node);
                self.label_names.push(name);
            }
        }
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
        if let Some(last) = &block.last_stmt {
            self.last_stmt(last);
        }
        for name in self.label_names.drain(outer_label_count..) {
            if let Some(nodes) = self.labels.get_mut(name) {
                nodes.pop();
            }
        }
        self.scope_end = outer_scope_end;
        self.run = None;
    }

    fn label_node(&self, token: &Token) -> Option<usize> {
        self.labels.get(token.identifier()?)?.last().copied()
    }

    fn stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::FunctionCall(chain) => {
                let node = self.simple(chain.span);
                if chain
                    .last_call()
                    .is_some_and(|call| is_global_call(call, "error", self.semantic))
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
            // Lua 5.2+: `break` as an ordinary statement.
            Statement::Break(span) => self.jump(*span, self.break_target),
            Statement::EmptyStatement(_) => {}
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

    fn last_stmt(&mut self, last: &'ast LastStatement) {
        match last {
            LastStatement::Return(node) => {
                let exit =
                    if node.exprs.is_empty() { Exit::ReturnEmpty } else { Exit::ReturnValue };
                self.jump(node.span, exit.node());
            }
            LastStatement::Break(span) => self.jump(*span, self.break_target),
            // Luau
            LastStatement::Continue(span) => self.jump(*span, self.continue_target),
            LastStatement::Error(span) => {
                self.code(*span);
            }
        }
    }

    fn loop_body(
        &mut self,
        block: &'ast Block,
        scope_end: u32,
        break_target: usize,
        continue_target: usize,
    ) {
        let outer_break_target = mem::replace(&mut self.break_target, break_target);
        let outer_continue_target = mem::replace(&mut self.continue_target, continue_target);
        self.scoped_block(block, scope_end);
        self.close_into(continue_target);
        self.break_target = outer_break_target;
        self.continue_target = outer_continue_target;
    }

    fn while_loop(&mut self, node: &'ast WhileLoop) {
        let head = self.code(Span::new(node.span.start, node.block.span.start));
        let after = self.new_node();
        let truth = literal_truth(&node.condition);
        if truth == Some(false) {
            self.cursor = None;
        }
        self.loop_body(&node.block, node.block.span.end, after, head);
        if truth != Some(true) {
            self.connect(head, after);
        }
        self.cursor = Some(after);
    }

    fn repeat_loop(&mut self, node: &'ast RepeatLoop) {
        let body = self.new_node();
        self.advance_to(body);
        let condition = self.new_node();
        let after = self.new_node();
        // A repeat's condition still sees its body's locals.
        self.loop_body(&node.block, node.condition.span().end, after, condition);
        self.graph.code.push((node.condition.span(), condition));
        let truth = literal_truth(&node.condition);
        if truth != Some(true) {
            self.connect(condition, body);
        }
        if truth != Some(false) {
            self.connect(condition, after);
        }
        self.cursor = Some(after);
    }

    fn for_loop(&mut self, header: Span, body: &'ast Block) {
        self.code(header);
        let head = self.new_node();
        self.advance_to(head);
        let after = self.new_node();
        self.loop_body(body, body.span.end, after, head);
        // The iterator can run out before the first iteration.
        self.connect(head, after);
        self.cursor = Some(after);
    }

    fn if_stmt(&mut self, node: &'ast IfStatement) {
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

fn literal_truth(condition: &Expression) -> Option<bool> {
    match unwrap_parentheses(condition) {
        Expression::Nil(_) | Expression::False(_) => Some(false),
        Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::InterpolatedString(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_) => Some(true),
        Expression::VarArg(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => None,
    }
}

pub(crate) fn decision_points(block: &Block) -> u32 {
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

impl<'ast> Visitor<'ast> for DecisionPoints {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::IfStatement(node) => self.count_clauses(node.elseif_clauses.len()),
            Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_) => self.count += 1,
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
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
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::BinaryOp(node) => {
                if matches!(node.links.first().op, BinOp::And | BinOp::Or) {
                    self.count += u32::try_from(node.links.len()).unwrap_or(u32::MAX);
                }
            }
            // Luau
            Expression::IfExpression(node) => self.count_clauses(node.elseif_clauses.len()),
            Expression::Nil(_)
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
            | Expression::UnaryOp(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => {}
        }
        self.walk_expr(expr);
    }

    fn visit_function_body(&mut self, _body: &'ast FunctionBody) {}

    fn visit_type(&mut self, _type_value: &'ast Type) {}
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use luck_token::LuaVersion;

    use super::*;

    fn parse_block(source: &str, version: LuaVersion) -> Block {
        let parse = luck_parser::parse(source, version);
        assert!(parse.errors.is_empty(), "parser errors for {source:?}: {:?}", parse.errors);
        parse.block
    }

    fn flow_of(source: &str, version: LuaVersion) -> FlowGraph {
        let block = parse_block(source, version);
        FlowGraph::of_block(&block, &luck_semantic::analyze(&block, version))
    }

    fn flow(source: &str) -> FlowGraph {
        flow_of(source, LuaVersion::Lua54)
    }

    fn points(source: &str) -> u32 {
        decision_points(&parse_block(source, LuaVersion::Lua54))
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
        let block_between = "local a
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
        let graph = flow("local c = 1\nif c then return 1 end");
        assert!(graph.reaches(Exit::End));
        assert!(graph.reaches(Exit::ReturnValue));
    }

    #[test]
    fn if_else_both_returning_never_fall_through() {
        let graph = flow("local c = 1\nif c then return 1 else return 2 end");
        assert!(graph.reaches(Exit::ReturnValue));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn if_elseif_else_all_returning_never_fall_through() {
        let graph = flow(
            "local c = 1\nlocal d = 1\nif c then return 1 elseif d then return 2 else return 3 end",
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
        let graph = flow("local c = 1\nwhile true do if c then break end end");
        assert!(graph.reaches(Exit::End));
        assert!(!graph.reaches(Exit::Break), "the `break` belongs to the loop");
    }

    #[test]
    fn while_with_truthy_literal_condition_never_falls_through() {
        for condition in ["1", "'s'", "{}", "(true)"] {
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
        let graph = flow("local function error() end\nerror(\"x\")");
        assert!(!graph.reaches(Exit::Error));
        assert!(graph.reaches(Exit::End));
    }

    #[test]
    fn break_in_inner_loop_is_consumed() {
        let graph = flow("local c = 1\nwhile c do if c then break end end");
        assert!(graph.reaches(Exit::End));
        assert!(!graph.reaches(Exit::Break));
    }

    #[test]
    fn continue_runs_the_until_condition() {
        let graph = flow_of("repeat continue until true\nreturn 2", LuaVersion::Luau);
        assert!(graph.reaches(Exit::ReturnValue));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn continue_of_the_analyzed_block_ends_it() {
        let block = parse_block("for _ = 1, 3 do continue end", LuaVersion::Luau);
        let Some(Statement::NumericFor(numeric_for)) = block.stmts.first() else {
            panic!("the statement must be a numeric for");
        };
        let semantic = luck_semantic::analyze(&block, LuaVersion::Luau);
        assert!(FlowGraph::of_block(&numeric_for.block, &semantic).reaches(Exit::End));
    }

    #[test]
    fn goto_jumps_over_the_code_before_its_label() {
        let graph = flow("goto skip\ndo return end\n::skip::\nprint(1)");
        assert!(!graph.reaches(Exit::ReturnEmpty));
        assert!(graph.reaches(Exit::End));
    }

    #[test]
    fn backward_goto_loops() {
        let graph = flow("::top::\nprint(1)\ngoto top");
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn goto_to_a_label_outside_the_block_leaves_it() {
        let block = parse_block("while true do goto done end\n::done::", LuaVersion::Lua54);
        let Some(Statement::WhileLoop(while_loop)) = block.stmts.first() else {
            panic!("the statement must be a while loop");
        };
        let semantic = luck_semantic::analyze(&block, LuaVersion::Lua54);
        let graph = FlowGraph::of_block(&while_loop.block, &semantic);
        assert!(graph.reaches(Exit::Break));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn goto_takes_the_innermost_label_of_its_name() {
        // The outer `::a::` would loop forever.
        let graph = flow("::a::\ndo\n  goto a\n  do return end\n  ::a::\nend");
        assert!(graph.reaches(Exit::End));
        assert!(!graph.reaches(Exit::ReturnEmpty));
    }

    #[test]
    fn label_goes_out_of_sight_with_its_block() {
        let graph = flow("do ::a:: end\ngoto a");
        assert!(graph.reaches(Exit::Break));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn gotos_find_their_labels_among_many() {
        let mut source = String::new();
        for i in 0..2_000u32 {
            let _unused = writeln!(source, "goto l{i}\ndo return end\n::l{i}::");
        }
        let graph = flow(&source);
        assert!(graph.reaches(Exit::End));
        assert!(!graph.reaches(Exit::ReturnEmpty));
    }

    #[test]
    fn can_run_is_false_after_a_jump_until_a_reached_label() {
        let source = "goto a\nprint(1)\n::a::\nprint(2)";
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
        assert_eq!(points("local c = 1\nif c then end"), 1);
    }

    #[test]
    fn decision_points_if_with_and() {
        // +1 for the if, +1 for the `and`.
        assert_eq!(points("local a = 1\nlocal b = 1\nif a and b then end"), 2);
    }

    #[test]
    fn decision_points_if_elseif() {
        // +1 for the if, +1 for the elseif.
        assert_eq!(points("local a = 1\nlocal b = 1\nif a then elseif b then end"), 2);
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
                "local a = 1\nlocal b = 1\nlocal c = 1\nif a then elseif b then elseif c then end"
            ),
            3
        );
    }
}
