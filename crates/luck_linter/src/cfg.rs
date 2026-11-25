//! A function's control-flow graph, which every rule that asks for one
//! shares. Each graph covers one block and stops at nested function bodies,
//! whose flow is their own.

use std::cell::OnceCell;
use std::mem;

use luck_ast::{
    Block, ConditionalBinding, GetSpan as _, IfStatement, RepeatLoop, Statement, WhileLoop,
};
use luck_token::Span;
use luck_value::ToBoolean as _;

use crate::call::is_global_call;
use crate::context::ContextHost;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Off the block's end, which in a loop body starts the next iteration,
    /// as `continue` does.
    End,
    /// A `break` of the loop whose body the block is.
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
    /// only the `local` statement itself re-enters the scope.
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
        };
        let entry = builder.new_node();
        builder.cursor = Some(entry);
        builder.block(block);
        builder.close_into(Exit::End.node());

        let mut graph = builder.graph;
        let mut is_reachable = vec![false; graph.node_count()];
        let mut stack = vec![entry];
        while let Some(node) = stack.pop() {
            if !mem::replace(&mut is_reachable[node], true) {
                stack.extend(graph.successors(node));
            }
        }
        graph.is_reachable = is_reachable;
        graph
    }

    pub(crate) fn reaches(&self, exit: Exit) -> bool {
        self.is_reachable[exit.node()]
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
    /// as the graph of that body alone has them.
    pub(crate) fn loop_exits(&self, loop_start: u32) -> Option<Exits> {
        let index = self.loops.binary_search_by_key(&loop_start, |body| body.start).ok()?;
        Some(self.loop_exits.get_or_init(|| self.every_loop_s_exits())[index])
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
                if mem::replace(&mut visited_in[node], index) == index {
                    continue;
                }
                if !(body.entry..body.end).contains(&node) {
                    past_end.push(node);
                } else if node != body.entry
                    && let Ok(inner) = self.loops.binary_search_by_key(&node, |inner| inner.entry)
                {
                    stack.extend(&reached[inner]);
                } else {
                    stack.extend(self.successors(node));
                }
            }
            reached[index] = past_end;
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
                        // Else the loop's `break`.
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
        self.node_at(offset).is_none_or(|node| self.is_reachable[node])
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
        self.successors[node].into_iter().flatten()
    }

    pub(crate) fn locals(&self) -> &[LocalDeclaration] {
        &self.locals
    }

    pub(crate) fn starts_at_or_after(&self, node: usize, offset: u32) -> bool {
        self.code_starts[node] >= offset
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
}

impl<'ast> GraphBuilder<'_, 'ast> {
    fn new_node(&mut self) -> usize {
        self.graph.successors.push([None; 2]);
        self.graph.code_starts.push(0);
        self.graph.node_count() - 1
    }

    fn connect(&mut self, from: usize, to: usize) {
        if let Some(free_edge) = self.graph.successors[from].iter_mut().find(|edge| edge.is_none())
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
        self.graph.code_starts[node] = span.start;
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
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
        self.scope_end = outer_scope_end;
        self.run = None;
    }

    fn stmt(&mut self, stmt: &'ast Statement<'ast>) {
        match stmt {
            Statement::FunctionCall(chain) => {
                let node = self.simple(chain.span);
                if chain
                    .last_call()
                    .is_some_and(|call| is_global_call(call, "error", self.host.scoping()))
                {
                    self.connect(node, Exit::Error.node());
                    self.cursor = None;
                }
            }
            Statement::LocalAssignment(local) => {
                let node = self.simple(local.span);
                let scope_end = self.scope_end;
                self.graph.locals.extend(local.names.iter().map(|local_name| LocalDeclaration {
                    name_span: local_name.name.span,
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
            Statement::Return(node) => {
                let exit =
                    if node.exprs.is_empty() { Exit::ReturnEmpty } else { Exit::ReturnValue };
                self.jump(node.span, exit.node());
            }
            Statement::Break(span) => self.jump(*span, self.break_target),
            Statement::Continue(span) => self.jump(*span, self.continue_target),
            Statement::Assignment(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
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
        self.graph.loops[index].end = end;
        entry
    }

    fn bind(&mut self, binding: Option<&ConditionalBinding<'_>>, node: usize, scope_end: u32) {
        if let Some(binding) = binding {
            let name_span = binding.name.name.span;
            self.graph.locals.push(LocalDeclaration { name_span, node, scope_end });
        }
    }

    fn while_loop(&mut self, node: &'ast WhileLoop<'ast>) {
        let head = self.code(Span::new(node.span.start, node.block.span.start));
        self.bind(node.binding.as_deref(), head, node.block.span.end);
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
        self.bind(node.binding.as_deref(), test, node.block.span.end);
        let join = self.new_node();
        self.block(&node.block);
        self.close_into(join);
        for clause in &node.elseif_clauses {
            self.cursor = Some(test);
            test = self.code(Span::new(clause.span.start, clause.block.span.start));
            self.bind(clause.binding.as_deref(), test, clause.block.span.end);
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

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use luck_ast::{FunctionBody, GenericFor, NumericFor, Visit};

    use super::*;
    use crate::fixtures::with_host;

    fn flow(source: &str) -> FlowGraph {
        with_host(source, |host| FlowGraph::of_block(host.chunk, host))
    }

    #[test]
    fn a_straight_run_of_statements_is_one_node() {
        let one = flow("local a").node_count();
        let mut run = String::from("local a, b");
        for i in 0..100 {
            write!(
                run,
                "
a = {i}
b = f(a)
print(a, b)"
            )
            .unwrap();
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
        let graph = flow(
            "\
repeat continue until true
return 2",
        );
        assert!(graph.reaches(Exit::ReturnValue));
        assert!(!graph.reaches(Exit::End));
    }

    #[test]
    fn continue_of_the_analyzed_block_ends_it() {
        let reaches_end = with_host("for _ = 1, 3 do continue end", |host| {
            let Some(Statement::NumericFor(numeric_for)) = host.chunk.stmts.first() else {
                panic!("the statement must be a numeric for");
            };
            FlowGraph::of_block(&numeric_for.block, host).reaches(Exit::End)
        });
        assert!(reaches_end);
    }

    #[test]
    fn break_of_the_analyzed_block_leaves_it() {
        let graph = with_host("while true do if x then break end end", |host| {
            let Some(Statement::WhileLoop(while_loop)) = host.chunk.stmts.first() else {
                panic!("the statement must be a while loop");
            };
            FlowGraph::of_block(&while_loop.block, host)
        });
        assert!(graph.reaches(Exit::Break));
        assert!(graph.reaches(Exit::End));
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
            "for i = 1, 2 do while true do break end end",
            "for i = 1, 2 do for j = 1, 2 do if j then break end continue end end",
            "for i = 1, 2 do while i do if i then continue end return end end",
            "while x do repeat if y then break end until true end",
            "while false do for i = 1, 2 do break end return end",
            "for i = 1, 2 do while true do end repeat until false end",
            "for i = 1, 2 do local f = function() while x do error() end end end",
            "for i = 1, 2 do repeat if x then continue end return until x end",
            "while x do for _ in t do if x then break end return end continue end",
        ];
        for source in sources {
            with_host(source, |host| {
                let graph = FlowGraph::of_block(host.chunk, host);
                let mut bodies = LoopBodies::default();
                bodies.visit_block(host.chunk);
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
    fn can_run_is_false_after_every_path_leaves() {
        let source = "\
do
  if x then return end
  print(1)
end
do return end
print(2)";
        let graph = flow(source);
        let offset_of = |text: &str| u32::try_from(source.find(text).expect("in source")).unwrap();
        assert!(graph.can_run(offset_of("print(1)")));
        assert!(!graph.can_run(offset_of("print(2)")));
    }
}
