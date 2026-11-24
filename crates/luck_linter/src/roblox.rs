use luck_ast::{Call, Expression, FunctionArgs};
use luck_stdlib::SemanticAnalysis;

use crate::call::dotted_callee;

/// The arguments when `call` is a parenthesized `<global>.new(...)` call,
/// and `<global>` isn't shadowed by a local.
pub(crate) fn global_new_call<'ast>(
    call: Call<'ast>,
    global: &str,
    semantic: &SemanticAnalysis<'_>,
) -> Option<&'ast [Expression<'ast>]> {
    let (root, field) = dotted_callee(call)?;
    if field != "new" || root.text != global {
        return None;
    }
    if semantic.scoping().resolves_to_local(root) {
        return None;
    }
    let FunctionArgs::Parenthesized { args, .. } = call.args else {
        return None;
    };
    Some(args)
}
