use std::{mem, vec};

use luck_allocator::{Allocator, List};
use luck_ast::{Block, Chunk, Expression, Statement, VisitMut};
use luck_diagnostics::FileDiagnostics;
use luck_token::chunk_header;

use crate::graph::DependencyGraph;
use crate::plan::{Plan, may_overflow};
use crate::rewrite::{LoaderNames, overflow_errors, rewrite_module};
use crate::text_emitter::{self, HOLE};

/// The bundle as a tree, and the lines its printed form opens with: the
/// entry's shebang and its hot comments, which take effect only there.
///
/// Each module keeps the tree its own parse gave it, rewritten where the text
/// splices, and everything around them is the tree the text parses to, which
/// goes in `allocator` with every node the rewrites build. A module's spans
/// index its own source, so two modules' spans can overlap.
pub(crate) fn emit<'ast>(
    allocator: &'ast Allocator,
    graph: &mut DependencyGraph<'ast>,
) -> Result<(Chunk<'ast>, Vec<u8>), Vec<FileDiagnostics>> {
    let entry_id = graph.entry_id;
    let entry = &graph[entry_id];
    let header = chunk_header(entry.source, entry.shebang, entry.hot_comments.iter().copied());
    if graph.modules.len() == 1 && entry.dependencies.is_empty() {
        let block = graph[entry_id].block.take();
        return Ok((Chunk::new(block), header));
    }

    let plan = Plan::new(graph);
    let names = LoaderNames::new(allocator, &plan.prefix);
    let mut bodies = Vec::with_capacity(graph.modules.len());
    let mut errors = Vec::new();
    for wrapped in plan.wrapped_modules() {
        let module = &mut graph[wrapped.id];
        let mut block = module.block.take();
        rewrite_module(allocator, &mut block, module, &plan, &names);
        if may_overflow(module) {
            errors.extend(overflow_errors(&block, module));
        }
        bodies.push(block);
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let skeleton = allocator.alloc_slice_copy(&text_emitter::skeleton(graph, &plan));
    let mut chunk = luck_parser::parse(allocator, skeleton).chunk;
    let mut filler = Filler { allocator, prefix: &plan.prefix, bodies: bodies.into_iter() };
    filler.visit_chunk(&mut chunk);
    Ok((chunk, header))
}

/// Puts each module's body, in bundle order, in place of the hole the
/// skeleton's parse holds for it.
struct Filler<'plan, 'ast> {
    allocator: &'ast Allocator,
    prefix: &'plan str,
    bodies: vec::IntoIter<Block<'ast>>,
}

impl Filler<'_, '_> {
    fn is_hole(&self, stmt: &Statement<'_>) -> bool {
        let Statement::FunctionCall(call) = stmt else {
            return false;
        };
        matches!(&call.prefix, Expression::Name(name) if name.text.strip_prefix(self.prefix) == Some(HOLE))
    }
}

impl<'ast> VisitMut<'ast> for Filler<'_, 'ast> {
    /// A block's own statements are walked before its hole is filled, so no
    /// module's are. Only an export table's `return` follows a hole, and a
    /// module with value exports has no `return` of its own.
    fn visit_block(&mut self, block: &mut Block<'ast>) {
        self.walk_block(block);
        let Some(hole) = block.stmts.iter().position(|stmt| self.is_hole(stmt)) else {
            return;
        };
        let mut body = self.bodies.next().expect("each hole has its module's body");
        let capacity = block.stmts.len() + body.stmts.len();
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
