//! A run of requires prints sorted by the name each binds.
//!
//! A run is two or more sortable statements a block prints one after another, outside any
//! `-- luck: format off` region, with no blank line or comment line between them. A statement is
//! sortable only if its lines hold nothing that can't move with it. A statement under
//! `luck: ignore`, a `<close>` binding, a trailing comment that spans lines, or any other statement
//! ends a run, and a run whose members read each other's bindings stays as written.
//!
//! The block emitter decides that as it prints, from the lines it writes, which are the lines a
//! second format reads, so it keeps the order the first one chose. It then moves each member's
//! printed lines, so comments and trivia on a line move with it.

use core::mem;
use core::ops::Range;
use core::slice;

use luck_ast::{Block, ChainHead, Expression, FunctionArgs, LocalAssignment, Statement, Suffix};
use luck_token::{Span, offset_to_index};
use rustc_hash::FxHashSet;

use crate::ir::{Formatter, SortedRun};

/// The spans of the blocks whose runs sort.
///
/// That's the chunk, and each block a statement in one of them opens. A
/// function's body counts only where a statement assigns the function itself,
/// so not one passed as a call argument, returned, or held in a table.
pub(crate) fn sorting_blocks(chunk: &Block) -> FxHashSet<Span> {
    let mut blocks = FxHashSet::default();
    let mut pending = vec![chunk];
    while let Some(block) = pending.pop() {
        blocks.insert(block.span);
        for stmt in &block.stmts {
            push_opened_blocks(stmt, &mut pending);
        }
    }
    blocks
}

fn push_opened_blocks<'block>(stmt: &'block Statement, pending: &mut Vec<&'block Block>) {
    let assigned_functions = |exprs: &'block [Expression]| {
        exprs.iter().filter_map(|expr| {
            if let Expression::FunctionDef(function) = expr {
                Some(&function.body.block)
            } else {
                None
            }
        })
    };
    match stmt {
        Statement::DoBlock(node) => pending.push(&node.block),
        Statement::WhileLoop(node) => pending.push(&node.block),
        Statement::RepeatLoop(node) => pending.push(&node.block),
        Statement::NumericFor(node) => pending.push(&node.block),
        Statement::GenericFor(node) => pending.push(&node.block),
        Statement::IfStatement(node) => {
            pending.push(&node.block);
            pending.extend(node.elseif_clauses.iter().map(|clause| &clause.block));
            pending.extend(node.else_clause.iter().map(|clause| &clause.block));
        }
        Statement::FunctionDecl(node) => pending.push(&node.body.block),
        Statement::LocalFunction(node) => pending.push(&node.body.block),
        // Lua 5.5
        Statement::GlobalFunction(node) => pending.push(&node.body.block),
        Statement::LocalAssignment(local) => {
            if let Some(exprs) = &local.exprs {
                pending.extend(assigned_functions(&exprs.items));
            }
        }
        Statement::Assignment(node) => pending.extend(assigned_functions(&node.values.items)),
        // Luau
        Statement::CompoundAssignment(node) => {
            pending.extend(assigned_functions(slice::from_ref(&node.expr)));
        }
        // Lua 5.5
        Statement::GlobalDeclaration(global) => {
            if let Some(exprs) = &global.exprs {
                pending.extend(assigned_functions(&exprs.items));
            }
        }
        Statement::FunctionCall(_)
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => {}
    }
}

/// What a sortable statement binds and reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Require<'block> {
    /// The binding name, used as the sort key.
    key: &'block str,
    /// The name the value reads: `require`, or the `GetService` receiver.
    callee: &'block str,
    /// The name a required path starts at, which the value reads too:
    /// `script` in `require(script.Parent.Module)`.
    path_root: Option<&'block str>,
}

/// Matches a `local` with one name bound to one require or `GetService` call,
/// in any dialect.
pub(crate) fn sortable_require(stmt: &Statement) -> Option<Require<'_>> {
    let Statement::LocalAssignment(local) = stmt else {
        return None;
    };
    let key = single_binding_name(local)?;
    let exprs = local.exprs.as_ref()?;
    let [value] = exprs.items.as_slice() else {
        return None;
    };
    let (callee, path_root) = pure_require_reads(value)?;
    Some(Require { key, callee, path_root })
}

struct Member<'block> {
    require: Require<'block>,
    /// The elements the formatter wrote for it, trailing comments included.
    elements: Range<usize>,
    /// The source those came from.
    source: Span,
}

/// The sortable statements a block has printed since the last thing that
/// ends a run.
#[derive(Default)]
pub(crate) struct RequireRun<'block> {
    members: Vec<Member<'block>>,
}

impl<'block> RequireRun<'block> {
    pub(crate) fn push(&mut self, require: Require<'block>, elements: Range<usize>, source: Span) {
        self.members.push(Member { require, elements, source });
    }

    /// Prints the run sorted when it has two or more members and sorting
    /// can't change what they read, then empties it.
    pub(crate) fn finish(&mut self, f: &mut Formatter) {
        let members = mem::take(&mut self.members);
        if members.len() < 2 {
            return;
        }
        let keys: FxHashSet<&str> = members.iter().map(|member| member.require.key).collect();
        let reads_a_member = members.iter().any(|member| {
            keys.contains(member.require.callee)
                || member.require.path_root.is_some_and(|root| keys.contains(root))
        });
        if reads_a_member {
            return;
        }
        let mut order: Vec<usize> = (0..members.len()).collect();
        order.sort_by_key(|&i| members.get(i).map(|member| member.require.key));
        if order.iter().enumerate().all(|(position, &i)| position == i) {
            return;
        }
        let slices: Vec<Range<usize>> =
            members.iter().map(|member| member.elements.clone()).collect();
        let sources = order.iter().filter_map(|&i| members.get(i)).map(|member| member.source);
        f.reorder(&slices, &order, SortedRun { members: sources.collect() });
    }
}

/// `source` with each of `runs` in the order it printed, one member per
/// line, which parses to the tree the formatted text does.
pub(crate) fn sorted_source(source: &str, runs: &[SortedRun]) -> String {
    let mut spans: Vec<(Span, &SortedRun)> = runs
        .iter()
        .filter_map(|run| {
            let start = run.members.iter().map(|member| member.start).min()?;
            let end = run.members.iter().map(|member| member.end).max()?;
            Some((Span::new(start, end), run))
        })
        .collect();
    spans.sort_by_key(|(span, _)| span.start);
    let mut sorted = String::with_capacity(source.len());
    let mut cursor = 0u32;
    for (span, run) in spans {
        sorted.push_str(Span::new(cursor, span.start).text(source));
        let lines: Vec<&str> = run.members.iter().map(|member| member.text(source)).collect();
        sorted.push_str(&lines.join("\n"));
        cursor = span.end;
    }
    sorted.push_str(source.get(offset_to_index(cursor)..).unwrap_or_default());
    sorted
}

/// The one name `local` binds, unless it's a `<close>` binding, whose
/// declaration order sets the order things close in.
fn single_binding_name(local: &LocalAssignment) -> Option<&str> {
    let [binding] = local.names.items.as_slice() else {
        return None;
    };
    if binding
        .attribute
        .as_deref()
        .is_some_and(|attribute| attribute.name.identifier() == Some("close"))
    {
        return None;
    }
    binding.name.identifier()
}

/// The names a sortable value reads: its callee, then the name a required
/// path starts at.
///
/// The value is `require(STRING)`, `require(NAME.field)` with any number of
/// fields, or `NAME:GetService(STRING)`, and a string argument may drop its
/// parentheses. Anything else, such as `require("a")()`, may have side
/// effects that sorting would reorder.
fn pure_require_reads(expr: &Expression) -> Option<(&str, Option<&str>)> {
    let Expression::Suffixed(chain) = expr else {
        return None;
    };
    let (Expression::Name(callee), [Suffix::Call(call)]) = (&chain.prefix, &*chain.suffixes) else {
        return None;
    };
    let (method, args) = (&call.method, &call.args);
    let callee_name = callee.identifier()?;

    if let Some(method_name) = method {
        let is_get_service =
            method_name.identifier() == Some("GetService") && is_single_string_arg(args);
        return is_get_service.then_some((callee_name, None));
    }
    if callee_name != "require" {
        return None;
    }
    if is_single_string_arg(args) {
        return Some((callee_name, None));
    }
    let FunctionArgs::Parenthesized { args: path_args, .. } = args else {
        return None;
    };
    let [path] = path_args.items.as_slice() else {
        return None;
    };
    let root = ChainHead::from(path).dotted_path()?.segments.first().copied()?;
    Some((callee_name, Some(root)))
}

fn is_single_string_arg(call_args: &FunctionArgs) -> bool {
    match call_args {
        FunctionArgs::Parenthesized { args, .. } => {
            args.len() == 1 && matches!(args.first(), Some(Expression::StringLiteral(_)))
        }
        FunctionArgs::StringLiteral(_) => true,
        FunctionArgs::TableConstructor(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::FormatOptions;

    fn sort_source(input: &str) -> String {
        let options = FormatOptions { sort_requires: true, ..FormatOptions::default() };
        let result = crate::format(input, LuaVersion::Lua54, &options);
        assert!(result.errors.is_empty(), "parse: {:?}", result.errors);
        result.output
    }

    #[test]
    fn sorts_simple_run() {
        let input = "local zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\nlocal mid = require(\"mid\")\n";
        let expected = "local alpha = require(\"alpha\")\nlocal mid = require(\"mid\")\nlocal zeta = require(\"zeta\")\n";
        assert_eq!(sort_source(input), expected);
    }

    #[test]
    fn blank_line_breaks_run() {
        let input = "local zeta = require(\"zeta\")\n\nlocal alpha = require(\"alpha\")\n";
        // The blank line leaves two one-statement runs, too short to sort.
        assert_eq!(sort_source(input), input);
    }

    #[test]
    fn non_require_breaks_run() {
        let input =
            "local zeta = require(\"zeta\")\nlocal x = 1\nlocal alpha = require(\"alpha\")\n";
        // The `local x = 1` line splits the run; no reordering.
        assert_eq!(sort_source(input), input);
    }

    #[test]
    fn multi_binding_is_unsortable() {
        let input =
            "local zeta = require(\"zeta\")\nlocal a, b = 1, 2\nlocal alpha = require(\"alpha\")\n";
        assert_eq!(sort_source(input), input);
    }

    #[test]
    fn side_effect_require_is_unsortable() {
        let input = "local zeta = require(\"zeta\")()\nlocal alpha = require(\"alpha\")()\n";
        assert_eq!(sort_source(input), input);
    }

    #[test]
    fn format_off_region_is_preserved() {
        let input = "-- luck: format off\nlocal zeta = require(\"zeta\")\nlocal alpha = require(\"alpha\")\n-- luck: format on\n";
        assert_eq!(sort_source(input), input);
    }

    #[test]
    fn mixed_require_and_get_service() {
        let input = "local zeta = require(\"zeta\")\nlocal alpha = game:GetService(\"Players\")\n";
        let expected =
            "local alpha = game:GetService(\"Players\")\nlocal zeta = require(\"zeta\")\n";
        assert_eq!(sort_source(input), expected);
    }

    #[test]
    fn sorts_requires_of_field_paths() {
        let input = "local Zebra = require(script.Parent.Zebra)\nlocal Apple = require(Packages.Apple)\nlocal Mango = require(module)\n";
        let expected = "local Apple = require(Packages.Apple)\nlocal Mango = require(module)\nlocal Zebra = require(script.Parent.Zebra)\n";
        assert_eq!(sort_source(input), expected);
    }

    #[test]
    fn require_of_a_path_rooted_in_the_run_is_unsortable() {
        let input =
            "local Packages = require(script.Packages)\nlocal Apple = require(Packages.Apple)\n";
        assert_eq!(sort_source(input), input);
    }

    #[test]
    fn require_of_a_computed_path_is_unsortable() {
        let called = "local Zebra = require(script:FindFirstChild(\"Zebra\"))\nlocal Apple = require(script.Apple)\n";
        assert_eq!(sort_source(called), called);
        let indexed = "local Zebra = require(script[name])\nlocal Apple = require(script.Apple)\n";
        assert_eq!(sort_source(indexed), indexed);
    }

    #[test]
    fn nested_function_body_run_sorts() {
        let input = "local function f()\n\tlocal zeta = require(\"zeta\")\n\tlocal alpha = require(\"alpha\")\nend\n";
        let expected = "local function f()\n\tlocal alpha = require(\"alpha\")\n\tlocal zeta = require(\"zeta\")\nend\n";
        assert_eq!(sort_source(input), expected);
    }

    #[test]
    fn trailing_comments_move_with_their_statements() {
        let input = "local zeta = require(\"zeta\") -- last\nlocal alpha = require(\"alpha\") --[[ first ]]\n";
        let expected = "local alpha = require(\"alpha\") --[[ first ]]\nlocal zeta = require(\"zeta\") -- last\n";
        assert_eq!(sort_source(input), expected);
    }

    #[test]
    fn statements_sharing_a_source_line_sort_as_the_lines_they_print() {
        let input = "local zeta = require(\"zeta\") local alpha = require(\"alpha\")\n";
        let expected = "local alpha = require(\"alpha\")\nlocal zeta = require(\"zeta\")\n";
        assert_eq!(sort_source(input), expected);
    }

    #[test]
    fn verification_reads_each_run_in_the_order_it_printed() {
        let options = FormatOptions { sort_requires: true, ..FormatOptions::default() };
        let input = "local zeta = require(\"zeta\") -- z\nlocal beta = require(\"beta\") local alpha = require(\"alpha\") --[[ a ]]\ndo\n\tlocal y = require(\"y\");local x = require(\"x\")\nend\n";
        let result = crate::format_and_verify(input, LuaVersion::Lua54, &options)
            .unwrap_or_else(|(_, diff)| panic!("{diff:?}"));
        assert_eq!(
            result.output,
            "local alpha = require(\"alpha\") --[[ a ]]\nlocal beta = require(\"beta\")\nlocal zeta = require(\"zeta\") -- z\ndo\n\tlocal x = require(\"x\")\n\tlocal y = require(\"y\")\nend\n"
        );
    }
}
