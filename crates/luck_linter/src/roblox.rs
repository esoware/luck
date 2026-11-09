//! Helpers for the Roblox rules that check `<Global>.new(...)` calls with
//! literal numeric arguments.

use luck_ast::{Expression, FunctionArgs, FunctionCall, Punctuated};
use luck_semantic::SemanticAnalysis;

use crate::call::dotted_callee;

/// The arguments when `call` is a parenthesized `<global>.new(...)` call under
/// the Roblox stdlib, and `<global>` isn't shadowed by a local.
pub(crate) fn global_new_call<'call>(
    call: &'call FunctionCall,
    global: &str,
    semantic: &SemanticAnalysis,
) -> Option<&'call Punctuated<Expression>> {
    if !semantic.environment.is_roblox() {
        return None;
    }
    let (root, field) = dotted_callee(call)?;
    if field != "new" || root.identifier() != Some(global) {
        return None;
    }
    if semantic.resolves_to_local(global, root.span) {
        return None;
    }
    let FunctionArgs::Parenthesized { args, .. } = &call.args else {
        return None;
    };
    Some(args)
}
