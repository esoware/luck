use luck_ast::{Call, Expression, FunctionBody, GetSpan as _, Statement};
use luck_semantic::ScopeTree;
use rustc_hash::FxHashMap;

use crate::call::{CallArg, CallArgs};
use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct MismatchedArgCount;

impl_rule!(
    MismatchedArgCount,
    "mismatched_arg_count",
    Correctness,
    Warning,
    "call passes more arguments than the called function accepts"
);

impl WholeRule for MismatchedArgCount {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let signatures = function_signatures(context);
        if signatures.is_empty() {
            return Vec::new();
        }
        let tree = context.semantic.scope_tree();
        context
            .nodes
            .as_slice()
            .iter()
            .filter_map(|node| check_call(node.call()?, tree, &signatures))
            .collect()
    }
}

#[derive(Clone, Copy)]
struct FunctionSignature {
    param_count: usize,
    has_vararg: bool,
}

impl FunctionSignature {
    fn of(body: &FunctionBody<'_>) -> Self {
        Self { param_count: body.params.len(), has_vararg: body.vararg.is_some() }
    }
}

/// Each local bound to a function literal, by where its name starts, with
/// that function's signature.
fn function_signatures(context: &LintContext<'_>) -> FxHashMap<u32, FunctionSignature> {
    let mut signatures = FxHashMap::default();
    for block in context.nodes.blocks() {
        for stmt in &block.stmts {
            match stmt {
                Statement::LocalFunction(local_function) => {
                    let signature = FunctionSignature::of(&local_function.body);
                    signatures.insert(local_function.name.span.start, signature);
                }
                Statement::LocalAssignment(local) => {
                    for (attributed, value) in local.names.iter().zip(&local.exprs) {
                        if let Expression::FunctionDef(function) = value {
                            let signature = FunctionSignature::of(&function.body);
                            signatures.insert(attributed.name.span.start, signature);
                        }
                    }
                }
                Statement::Assignment(_)
                | Statement::FunctionCall(_)
                | Statement::DoBlock(_)
                | Statement::WhileLoop(_)
                | Statement::RepeatLoop(_)
                | Statement::IfStatement(_)
                | Statement::NumericFor(_)
                | Statement::GenericFor(_)
                | Statement::FunctionDecl(_)
                | Statement::Return(_)
                | Statement::Goto(_)
                | Statement::Label(_)
                | Statement::GlobalDeclaration(_)
                | Statement::GlobalFunction(_)
                | Statement::GlobalStar(_)
                | Statement::Break(_)
                | Statement::Continue(_)
                | Statement::CompoundAssignment(_)
                | Statement::TypeDeclaration(_)
                | Statement::Error(_) => {}
            }
        }
    }
    signatures
}

/// A local that's ever reassigned is left out, since it could hold any
/// function by the time a call runs.
fn check_call(
    call: Call<'_>,
    tree: &ScopeTree<'_>,
    signatures: &FxHashMap<u32, FunctionSignature>,
) -> Option<Finding> {
    if call.method.is_some() {
        return None;
    }
    let callee = call.callee.as_name()?;
    let symbol = tree.symbol(tree.reference(tree.reference_of(callee)?).resolved?);
    if symbol.is_modified {
        return None;
    }
    let definition_start = symbol.definition_span.start;
    let signature = signatures.get(&definition_start)?;
    if signature.has_vararg {
        return None;
    }
    let args = CallArgs::new(call.args);
    let arg_count = args.len();
    // A trailing call or `...` can expand to nothing, so only the args
    // before it are sure to be passed.
    let has_multi_value_tail = args.last().is_some_and(CallArg::is_multi_value);
    let guaranteed_arg_count = arg_count.saturating_sub(usize::from(has_multi_value_tail));
    if guaranteed_arg_count <= signature.param_count {
        return None;
    }
    Some(
        Finding::new(
            format!(
                "call passes {guaranteed_arg_count} arguments to function `{}`, \
                 which accepts only {}",
                callee.text, signature.param_count
            ),
            call.span(),
        )
        .with_label(symbol.definition_span, "defined here")
        .with_help("extra arguments are silently discarded"),
    )
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::MismatchedArgCount;
    use crate::diagnostic::Message;
    use crate::fixtures::{labeled, run_rule};

    fn run(source: &str) -> Vec<Message> {
        run_rule(&MismatchedArgCount, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_extra_args_to_local_function() {
        let source = "\
local function f(a, b) return a + b end
f(1, 2, 3)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.ends_with("which accepts only 2"), "{diags:?}");
        assert_eq!(labeled(&diags[0], source), ["f"]);
    }

    #[test]
    fn flags_extra_args_to_function_expr() {
        let diags = run("\
local f = function(a) return a end
f(1, 2)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_zero_param_function() {
        let diags = run("\
local function f() end
f(1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_guaranteed_overflow_with_trailing_call() {
        // g() may yield nothing, but 1 and 2 already exceed one param.
        let diags = run("\
local function g() end
local function f(a) return a end
f(1, 2, g())");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_string_and_table_arguments_without_parentheses() {
        let diags = run("\
local function f() end
f\"x\"
f{}");
        assert_eq!(diags.len(), 2, "{diags:?}");
    }

    #[test]
    fn ignores_exact_arg_count() {
        let diags = run("\
local function f(a, b) return a + b end
f(1, 2)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_fewer_args() {
        // Missing args are nil, which is how Lua does optional arguments.
        let diags = run("\
local function f(a, b) return a or b end
f(1)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_vararg_function() {
        let diags = run("\
local function f(...) return ... end
f(1, 2, 3, 4)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_trailing_call_at_boundary() {
        // f(1, g()): g() may yield zero values, so 1 arg is guaranteed.
        let diags = run("\
local function g() end
local function f(a) return a end
f(1, g())");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_reassigned_function() {
        let diags = run("\
local function f(a) return a end
f = function(a, b, c) return a end
f(1, 2, 3)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_global_function_calls() {
        let diags = run("print(1, 2, 3)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_table_field_calls() {
        let diags = run("\
local t = { f = function(a) return a end }
t.f(1, 2)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_call_to_shadowing_local() {
        let diags = run("\
local function f(a) return a end
do local f = print
f(1, 2) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_recursive_call_within_limit() {
        let diags = run("\
local function f(a)
    if a > 0 then return f(a - 1) end
end
f(3)");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
