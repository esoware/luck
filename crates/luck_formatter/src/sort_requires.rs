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

use luck_ast::{Expression, FunctionArgs, Statement, Suffix};
use rustc_hash::FxHashSet;

use crate::ir::Formatter;

/// What a sortable statement binds and reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Require<'ast> {
    /// The binding name, used as the sort key.
    key: &'ast str,
    /// The name the value reads: `require`, or the `GetService` receiver.
    callee: &'ast str,
    /// The name a required path starts at, which the value reads too:
    /// `script` in `require(script.Parent.Module)`.
    path_root: Option<&'ast str>,
}

/// Matches a `local` with one name bound to one require or `GetService` call,
/// in any dialect.
pub(crate) fn sortable_require<'ast>(stmt: &Statement<'ast>) -> Option<Require<'ast>> {
    let Statement::LocalAssignment(local) = stmt else {
        return None;
    };
    // A `<close>` binding's declaration order sets the order things close in.
    let [binding] = &*local.names else {
        return None;
    };
    if binding.attribute.as_ref().is_some_and(|attribute| attribute.name.text == "close") {
        return None;
    }
    let [value] = &*local.exprs else {
        return None;
    };
    let (callee, path_root) = pure_require_reads(value)?;
    Some(Require { key: binding.name.text.as_str(), callee, path_root })
}

struct Member<'ast> {
    require: Require<'ast>,
    /// The elements the formatter wrote for it, trailing comments included.
    elements: Range<usize>,
}

/// The sortable statements a block has printed since the last thing that
/// ends a run.
#[derive(Default)]
pub(crate) struct RequireRun<'ast> {
    members: Vec<Member<'ast>>,
}

impl<'ast> RequireRun<'ast> {
    pub(crate) fn push(&mut self, require: Require<'ast>, elements: Range<usize>) {
        self.members.push(Member { require, elements });
    }

    /// Prints the run sorted when it has two or more members and sorting
    /// can't change what they read, then empties it.
    pub(crate) fn finish(&mut self, f: &mut Formatter<'_>) {
        if self.members.len() < 2 {
            self.members.clear();
            return;
        }
        let members = mem::take(&mut self.members);
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

/// The names a sortable value reads: its callee, then the name a required
/// path starts at.
///
/// The value is `require(STRING)`, `require(NAME.field)` with any number of
/// fields, or `NAME:GetService(STRING)`, and a string argument may drop its
/// parentheses. Anything else, such as `require("a")()`, may have side
/// effects that sorting would reorder.
fn pure_require_reads<'ast>(expr: &Expression<'ast>) -> Option<(&'ast str, Option<&'ast str>)> {
    let Expression::Suffixed(chain) = expr else {
        return None;
    };
    let Expression::Name(callee) = &chain.prefix else {
        return None;
    };
    match &*chain.suffixes {
        [Suffix::MethodCall(call)] => {
            let is_get_service = call.name.text == "GetService" && is_single_string_arg(&call.args);
            is_get_service.then_some((callee.text.as_str(), None))
        }
        [Suffix::Call(args)] if callee.text == "require" => {
            if is_single_string_arg(args) {
                return Some((callee.text.as_str(), None));
            }
            let FunctionArgs::Parenthesized { args: path_args, .. } = args else {
                return None;
            };
            let [path] = &**path_args else {
                return None;
            };
            Some((callee.text.as_str(), Some(dotted_path_root(path)?)))
        }
        _ => None,
    }
}

/// The name a `NAME(.NAME)*` path starts at.
fn dotted_path_root<'ast>(path: &Expression<'ast>) -> Option<&'ast str> {
    let (prefix, suffixes) = if let Expression::Suffixed(chain) = path {
        (&chain.prefix, &*chain.suffixes)
    } else {
        (path, [].as_slice())
    };
    let Expression::Name(root) = prefix else {
        return None;
    };
    suffixes.iter().all(|suffix| matches!(suffix, Suffix::Field(_))).then_some(root.text.as_str())
}

fn is_single_string_arg(call_args: &FunctionArgs<'_>) -> bool {
    match call_args {
        FunctionArgs::Parenthesized { args, .. } => {
            matches!(&**args, [Expression::StringLiteral(_)])
        }
        FunctionArgs::StringLiteral(_) => true,
        FunctionArgs::TableConstructor(_) | FunctionArgs::Error(_) => false,
    }
}
