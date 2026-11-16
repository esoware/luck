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

use luck_ast::{ChainHead, Expression, FunctionArgs, LocalAssignment, Statement, Suffix};
use rustc_hash::FxHashSet;

use crate::ir::Formatter;

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
}

/// The sortable statements a block has printed since the last thing that
/// ends a run.
#[derive(Default)]
pub(crate) struct RequireRun<'block> {
    members: Vec<Member<'block>>,
}

impl<'block> RequireRun<'block> {
    pub(crate) fn push(&mut self, require: Require<'block>, elements: Range<usize>) {
        self.members.push(Member { require, elements });
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
        let slices: Vec<Range<usize>> = members.into_iter().map(|member| member.elements).collect();
        f.reorder(&slices, &order);
    }
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
    let (method, args) = (call.method(), call.args());
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
        FunctionArgs::TableConstructor(_) | FunctionArgs::Error(_) => false,
    }
}
