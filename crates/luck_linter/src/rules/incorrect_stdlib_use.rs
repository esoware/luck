use luck_ast::{Expression, FunctionCall, Statement};
use luck_semantic::{ResolvedCallee, SemanticAnalysis, StdlibEntry, StdlibFunction};

use crate::call::{has_multi_value_tail, positional_args, string_literal_value};
use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, Rule};

/// How many allowed values a message lists. Generated sets (Roblox service
/// and class names) run to hundreds of values.
const LISTED_VALUE_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy)]
pub(crate) struct IncorrectStdlibUse;

impl Rule for IncorrectStdlibUse {
    fn name(&self) -> &'static str {
        "incorrect_stdlib_use"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "wrong argument count or invalid constant in a standard library call"
    }
}

impl NodeRule for IncorrectStdlibUse {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr])
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Statement::FunctionCall(call) = stmt {
            check_call(call, context.semantic, findings);
        }
    }
    fn on_expression(
        &self,
        expr: &Expression,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Expression::FunctionCall(call) = expr {
            check_call(call, context.semantic, findings);
        }
    }
}

fn check_call(call: &FunctionCall, semantic: &SemanticAnalysis, findings: &mut Vec<Finding>) {
    let Some(ResolvedCallee { name, entry }) = semantic.resolve_callee(call) else {
        return;
    };
    let StdlibEntry::Function(function) = entry else {
        return;
    };
    let (args, arg_count) = positional_args(&call.args);
    // A trailing call or `...` can expand to any number of values, so the
    // minimum and overload checks skip it.
    let has_multi_value_tail = has_multi_value_tail(args);

    let min_args = function.min_args();
    let arity_message = if arg_count < min_args && !has_multi_value_tail {
        Some(format!("`{name}` requires at least {min_args} argument(s), got {arg_count}"))
    } else if let Some(max_args) = function.max_args()
        && arg_count > max_args
    {
        Some(format!("`{name}` accepts at most {max_args} argument(s), got {arg_count}"))
    } else if !function.accepts_arg_count(arg_count) && !has_multi_value_tail {
        // Within the overall bounds but between overloads, like 1 argument
        // to a function that takes 0 or 2.
        Some(format!("no overload of `{name}` accepts {arg_count} argument(s)"))
    } else {
        None
    };
    match arity_message {
        Some(message) => {
            findings.push(Finding::new(message, call.span));
        }
        None => check_constant_args(&name, function, args, arg_count, findings),
    }
}

/// Flags a literal string argument that isn't one of the constants its
/// parameter allows.
///
/// With overloads, a value passes if any signature matching the arg count
/// allows it, and a position some matching signature leaves unconstrained
/// isn't checked.
fn check_constant_args(
    name: &str,
    function: &StdlibFunction,
    args: &[Expression],
    arg_count: usize,
    findings: &mut Vec<Finding>,
) {
    for (index, expr) in args.iter().enumerate() {
        let Some((value, span)) = string_literal_value(expr) else {
            continue;
        };
        let Some(allowed_sets) = function.allowed_constants(index, arg_count) else {
            continue;
        };
        let allowed = || allowed_sets.iter().flat_map(|values| values.iter());
        if allowed().any(|constant| constant.value == value) {
            continue;
        }
        let allowed_list = allowed()
            .take(LISTED_VALUE_LIMIT)
            .map(|constant| format!("`{}`", constant.value))
            .collect::<Vec<_>>()
            .join(", ");
        let allowed_count = allowed().count();
        let truncation_note = if allowed_count > LISTED_VALUE_LIMIT {
            format!(", ... ({allowed_count} allowed values)")
        } else {
            String::new()
        };
        let arg_number = index.saturating_add(1);
        findings.push(Finding::new(
            format!(
                "`{name}` argument {arg_number} must be one of \
                 {allowed_list}{truncation_note}, got `{value}`",
            ),
            span,
        ));
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::IncorrectStdlibUse;
    use crate::fixtures::{run_rule, run_rule_roblox};

    fn run(source: &str, version: LuaVersion) -> Vec<String> {
        run_rule(&IncorrectStdlibUse, source, version)
            .into_iter()
            .map(|diag| diag.message)
            .collect()
    }

    #[test]
    fn flags_too_few_args() {
        let messages = run("table.insert()", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|message| message.contains("requires at least 2")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_too_many_args() {
        let messages = run("type(1, 2, 3)", LuaVersion::Lua54);
        assert!(messages.iter().any(|message| message.contains("at most 1")), "{messages:?}");
    }

    #[test]
    fn ignores_correct_call() {
        let messages = run("table.insert(t, 1)", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_invalid_constant_string() {
        // `collectgarbage` first arg must be one of the option names.
        let messages = run("collectgarbage('frob')", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|message| message.contains("argument 1 must be")),
            "{messages:?}"
        );
    }

    #[test]
    fn ignores_valid_constant_string() {
        let messages = run("collectgarbage('collect')", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_method_call() {
        // `obj` has no known shape, so `foo` doesn't resolve to the stdlib.
        let messages = run("obj:foo()", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_every_valid_cframe_new_overload() {
        // Every `CFrame.new` overload: no arguments, one or two `Vector3`s,
        // and 3, 7, or 12 numbers.
        let source = "local v = Vector3.new(1, 2, 3)\n\
                      local a = CFrame.new()\n\
                      local b = CFrame.new(v)\n\
                      local c = CFrame.new(v, v)\n\
                      local d = CFrame.new(1, 2, 3)\n\
                      local e = CFrame.new(1, 2, 3, 4, 5, 6, 7)\n\
                      local f = CFrame.new(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12)";
        let diags = run_rule_roblox(&IncorrectStdlibUse, source);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_cframe_new_between_overload_arities() {
        let diags = run_rule_roblox(&IncorrectStdlibUse, "local c = CFrame.new(1, 2, 3, 4, 5)");
        assert!(!diags.is_empty(), "5 args matches no CFrame.new overload: {diags:?}");
    }

    #[test]
    fn ignores_valid_udim2_overloads() {
        let source = "local u = UDim.new(0, 10)\n\
                      local a = UDim2.new(0, 0, 1, 0)\n\
                      local b = UDim2.new(u, u)";
        let diags = run_rule_roblox(&IncorrectStdlibUse, source);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_collectgarbage_param_form_in_55() {
        // The 5.5 three-arg tuning form matches only the `param`
        // overload; the plain-option overloads must not reject it.
        let messages = run("collectgarbage('param', 'pause', 100)", LuaVersion::Lua55);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_collectgarbage_incremental_tuning_in_54() {
        let messages = run("collectgarbage('incremental', 100, 200, 13)", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn ignores_shadowed_base() {
        let messages = run("local type = f\ntype(1, 2, 3)", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_file_method_missing_args() {
        let messages = run("local f = io.open('x')\nf:setvbuf()", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|message| message.contains("requires at least 1")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_file_method_bad_constant() {
        let messages = run("local f = io.open('x')\nf:seek('nope')", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|message| message.contains("argument 1 must be")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_string_literal_method_missing_args() {
        let messages = run("local s = ('x'):rep()", LuaVersion::Lua54);
        assert!(
            messages.iter().any(|message| message.contains("requires at least 1")),
            "{messages:?}"
        );
    }

    #[test]
    fn ignores_method_on_unshaped_local() {
        let messages = run("local f = something()\nf:setvbuf()", LuaVersion::Lua54);
        assert!(messages.is_empty(), "{messages:?}");
    }

    fn run_roblox(source: &str) -> Vec<String> {
        run_rule_roblox(&IncorrectStdlibUse, source).into_iter().map(|diag| diag.message).collect()
    }

    #[test]
    fn flags_get_service_typo() {
        let messages = run_roblox("game:GetService('Playerz')");
        assert!(
            messages.iter().any(|message| message.contains("argument 1 must be")
                && message.contains("allowed values")),
            "{messages:?}"
        );
    }

    #[test]
    fn ignores_valid_service_name() {
        let messages = run_roblox("game:GetService('ProximityPromptService')");
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_instance_new_unknown_class() {
        let messages = run_roblox("Instance.new('Playerz')");
        assert!(
            messages.iter().any(|message| message.contains("argument 1 must be")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_instance_new_abstract_class() {
        // `BasePart` exists but isn't creatable, so `IsA` accepts it and
        // `Instance.new` doesn't.
        let instance_new_messages = run_roblox("Instance.new('BasePart')");
        assert!(
            instance_new_messages.iter().any(|message| message.contains("argument 1 must be")),
            "{instance_new_messages:?}"
        );
        let is_a_messages = run_roblox("local x = script:IsA('BasePart')");
        assert!(is_a_messages.is_empty(), "{is_a_messages:?}");
    }

    #[test]
    fn ignores_valid_instance_new() {
        let messages = run_roblox("Instance.new('Folder')");
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn flags_get_service_arity() {
        let messages = run_roblox("game:GetService()");
        assert!(
            messages.iter().any(|message| message.contains("requires at least 1")),
            "{messages:?}"
        );
    }

    #[test]
    fn flags_brickcolor_unknown_name() {
        let unknown_name_messages = run_roblox("BrickColor.new('Bright rad')");
        assert!(
            unknown_name_messages.iter().any(|message| message.contains("argument 1 must be")),
            "{unknown_name_messages:?}"
        );
        let known_name_messages = run_roblox("BrickColor.new('Bright red')");
        assert!(known_name_messages.is_empty(), "{known_name_messages:?}");
        // The 3-number form skips the constant check entirely.
        let number_form_messages = run_roblox("BrickColor.new(1, 0, 0)");
        assert!(number_form_messages.is_empty(), "{number_form_messages:?}");
    }
}
