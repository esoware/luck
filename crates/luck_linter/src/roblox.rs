use luck_ast::{Call, Expression, FunctionArgs};
use luck_semantic::SemanticAnalysis;

use crate::call::dotted_callee;

/// The arguments when `call` is a parenthesized `<global>.new(...)` call under
/// the Roblox stdlib, and `<global>` isn't shadowed by a local.
pub(crate) fn global_new_call<'ast>(
    call: Call<'ast>,
    global: &str,
    semantic: &SemanticAnalysis<'_>,
) -> Option<&'ast [Expression<'ast>]> {
    if !semantic.environment().is_roblox() {
        return None;
    }
    let (root, field) = dotted_callee(call)?;
    if field != "new" || root.text != global {
        return None;
    }
    if semantic.scope_tree().resolves_to_local(root) {
        return None;
    }
    let FunctionArgs::Parenthesized { args, .. } = call.args else {
        return None;
    };
    Some(args)
}
