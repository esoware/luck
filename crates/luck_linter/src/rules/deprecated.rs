use luck_ast::{DottedPath, Expression, FunctionArgs, FunctionCall, Var, dotted_path};
use luck_semantic::{
    ResolvedCallee, SemanticAnalysis, StdlibDeprecation, StdlibEntry, StdlibFunction,
    expand_replace_template,
};

use crate::call::{positional_args, string_literal_value};
use crate::diagnostic::Finding;
use crate::fix::{Fix, TextEdit};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallPosition, LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Deprecated;

impl_rule!(
    Deprecated,
    "deprecated",
    Correctness,
    Warning,
    "use of a deprecated standard library function, argument, or value"
);

impl NodeRule for Deprecated {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[
            NodeType::FunctionCallStmt,
            NodeType::FunctionCallExpr,
            NodeType::Var,
        ])
    }
    fn on_call(
        &self,
        call: &FunctionCall,
        position: CallPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        check_call(call, position, context, findings);
    }
    fn on_expr(&self, expr: &Expression, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        match expr {
            Expression::Var(var) => findings.extend(check_field_access(var, context.semantic)),
            Expression::FunctionCall(_)
            | Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_) => {}
        }
    }
}

fn check_call(
    call: &FunctionCall,
    position: CallPosition,
    context: &LintContext<'_>,
    findings: &mut Vec<Finding>,
) {
    let Some(callee) = context.semantic.resolve_callee(call) else {
        return;
    };
    let entry = callee.entry();

    if let Some(deprecation) = entry.deprecation() {
        let name = callee.name();
        let fix = replacement_fix(call, position, &name, deprecation, context);
        findings.push(
            Finding::new(format!("`{name}` is deprecated"), call.span)
                .with_help(deprecation.message.as_str())
                .with_optional_fix(fix),
        );
        return;
    }

    if let StdlibEntry::Function(function) = entry {
        findings.extend(deprecated_constants(call, &callee, function));
        findings.extend(deprecated_params(call, &callee, function));
    }
}

/// Flags each argument in a position that every signature matching the
/// call's arity marks deprecated, like the `parent` of `Instance.new`.
///
/// Any expression counts, since passing that argument at all is the problem.
fn deprecated_params(
    call: &FunctionCall,
    callee: &ResolvedCallee<'_>,
    function: &StdlibFunction,
) -> impl Iterator<Item = Finding> {
    let (args, arg_count) = positional_args(&call.args);
    args.iter().enumerate().filter_map(move |(index, expr)| {
        let mut deprecations = function
            .matching_signatures(arg_count)
            .map(|signature| signature.params.get(index)?.deprecated.as_ref());
        // The first signature's deprecation speaks for all of them.
        let deprecation = deprecations.next().flatten()?;
        deprecations.all(|later| later.is_some()).then(|| {
            let arg_number = index.saturating_add(1);
            Finding::new(
                format!("argument {arg_number} of `{}` is deprecated", callee.name()),
                expr.span(),
            )
            .with_help(deprecation.message.as_str())
        })
    })
}

/// Flags each literal string argument that's a deprecated constant, like a
/// dead service name in `game:GetService(...)` or `collectgarbage("setpause")`
/// in 5.4.
///
/// Only positions that every signature matching the call's arity limits to
/// constants are checked.
fn deprecated_constants(
    call: &FunctionCall,
    callee: &ResolvedCallee<'_>,
    function: &StdlibFunction,
) -> impl Iterator<Item = Finding> {
    let (args, arg_count) = positional_args(&call.args);
    args.iter().enumerate().filter_map(move |(index, expr)| {
        let (value, span) = string_literal_value(expr)?;
        // Each signature's first constant spelled `value` counts, and the
        // first of those that's deprecated decides.
        let deprecation = function
            .allowed_constants(index, arg_count)?
            .iter()
            .filter_map(|values| values.iter().find(|constant| constant.value == value))
            .find_map(|constant| constant.deprecated.as_ref())?;
        Some(
            Finding::new(
                format!("`{value}` is a deprecated argument of `{}`", callee.name()),
                span,
            )
            .with_help(deprecation.message.as_str()),
        )
    })
}

/// The deprecation's replacement template filled in with the call's argument
/// text, or `None` when there's no template or the result can't stand where
/// the call does.
fn replacement_fix(
    call: &FunctionCall,
    position: CallPosition,
    display_name: &str,
    deprecation: &StdlibDeprecation,
    context: &LintContext<'_>,
) -> Option<Fix> {
    let template = deprecation.replace_template.as_ref()?;
    let replacement = expand_replace_template(template, &arg_texts(&call.args, context.source));
    // A template like `(%1 ^ %2)` expands to an expression, which can't
    // stand alone as a statement. Keep the diagnostic and drop the fix.
    if position == CallPosition::Statement
        && !luck_parser::parse(&replacement, context.semantic.version).errors.is_empty()
    {
        return None;
    }
    Some(Fix {
        description: format!("replace deprecated `{display_name}` with `{replacement}`"),
        edits: vec![TextEdit { span: call.span, replacement }],
    })
}

/// The source text of each argument, as a replacement template takes it.
fn arg_texts<'source>(function_args: &FunctionArgs, source: &'source str) -> Vec<&'source str> {
    match function_args {
        FunctionArgs::Parenthesized { args, .. } => {
            args.iter().map(|expr| expr.span().text(source)).collect()
        }
        FunctionArgs::StringLiteral(literal) => vec![literal.span.text(source)],
        FunctionArgs::TableConstructor(table) => vec![table.span.text(source)],
    }
}

/// Flags a dotted read that resolves to a deprecated constant, property, or
/// namespace, such as a deprecated Roblox enum item or type.
///
/// Functions are skipped because `check_call` reports them at the call.
fn check_field_access(var: &Var, semantic: &SemanticAnalysis) -> Option<Finding> {
    let Var::FieldAccess(_) = var else {
        return None;
    };
    let DottedPath { segments, root_span, leaf_span } = dotted_path(var)?;
    let entry = semantic.stdlib().lookup(&segments)?;
    if matches!(entry, StdlibEntry::Function(_)) {
        return None;
    }
    // Check only this node's own entry. The inner access node reports a
    // deprecated prefix, so a chain warns once.
    let deprecation = entry.deprecation()?;
    if semantic.resolves_to_local(segments.first()?, root_span) {
        return None;
    }
    Some(
        Finding::new(format!("`{}` is deprecated", segments.join(".")), leaf_span)
            .with_help(deprecation.message.as_str()),
    )
}

#[cfg(test)]
mod tests {
    use core::slice;

    use luck_token::LuaVersion;

    use super::Deprecated;
    use crate::fixtures::{fix_and_reparse, run_rule, run_rule_roblox};

    /// Applies the first fix and returns its description and the new
    /// source, or `None` if there's no fix.
    fn run_fix(source: &str, version: LuaVersion) -> Option<(String, String)> {
        let diags = run_rule(&Deprecated, source, version);
        let diag = diags.into_iter().find(|diag| diag.fix.is_some())?;
        let applied = fix_and_reparse(source, slice::from_ref(&diag), version);
        Some((diag.fix?.description, applied))
    }

    #[test]
    fn flags_and_fixes_loadstring_to_load() {
        let (description, after) = run_fix("loadstring('x')", LuaVersion::Lua54).unwrap();
        assert_eq!(description, "replace deprecated `loadstring` with `load('x')`");
        assert_eq!(after, "load('x')");
    }

    #[test]
    fn flags_and_fixes_string_gfind_to_gmatch() {
        let (description, after) = run_fix("string.gfind(s, p)", LuaVersion::Lua54).unwrap();
        assert_eq!(description, "replace deprecated `string.gfind` with `string.gmatch(s, p)`");
        assert_eq!(after, "string.gmatch(s, p)");
    }

    #[test]
    fn flags_and_fixes_table_getn_to_length() {
        let (description, after) = run_fix("table.getn(t)", LuaVersion::Lua54).unwrap();
        assert_eq!(description, "replace deprecated `table.getn` with `#t`");
        assert_eq!(after, "#t");
    }

    #[test]
    fn flags_and_fixes_unpack_to_table_unpack() {
        let (description, after) = run_fix("unpack(t)", LuaVersion::Lua54).unwrap();
        assert_eq!(description, "replace deprecated `unpack` with `table.unpack(t)`");
        assert_eq!(after, "table.unpack(t)");
    }

    #[test]
    fn flags_and_fixes_math_pow_to_caret() {
        let (description, after) = run_fix("local r = math.pow(a, b)", LuaVersion::Lua54).unwrap();
        assert_eq!(description, "replace deprecated `math.pow` with `(a ^ b)`");
        assert_eq!(after, "local r = (a ^ b)");
    }

    #[test]
    fn flags_expr_template_in_stmt_position_without_fix() {
        // `(a ^ b)` can't stand alone as a statement, so there's no fix.
        let diags = run_rule(&Deprecated, "math.pow(a, b)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "expected no fix: {diags:?}");
        assert!(run_fix("math.pow(a, b)", LuaVersion::Lua54).is_none());
    }

    #[test]
    fn flags_and_fixes_math_log10_to_log_base_10() {
        let (description, after) = run_fix("math.log10(x)", LuaVersion::Lua54).unwrap();
        assert_eq!(description, "replace deprecated `math.log10` with `math.log(x, 10)`");
        assert_eq!(after, "math.log(x, 10)");
    }

    #[test]
    fn ignores_method_call() {
        // The type of `obj` is unknown, so `getn` doesn't resolve to a
        // stdlib entry.
        let diags = run_rule(&Deprecated, "obj:getn()", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_deprecated_string_method_on_literal() {
        // A string literal has the `string` shape, so `:gfind` warns like
        // `string.gfind`.
        let diags = run_rule(&Deprecated, "local it = ('x'):gfind('a')", LuaVersion::Lua51);
        assert!(diags.iter().any(|diag| diag.message.contains("gfind")), "{diags:?}");
    }

    #[test]
    fn flags_dead_service_name() {
        let diags = run_rule_roblox(&Deprecated, "local p = game:GetService('PointsService')");
        assert!(
            diags
                .iter()
                .any(|diag| diag.message.contains("`PointsService` is a deprecated argument")),
            "{diags:?}"
        );
    }

    #[test]
    fn ignores_live_service_name() {
        let diags = run_rule_roblox(&Deprecated, "local p = game:GetService('Players')");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_deprecated_instance_new_parent_arg() {
        let diags = run_rule_roblox(&Deprecated, "local p = Instance.new('Part', workspace)");
        assert!(
            diags.iter().any(|diag| diag.message.contains("argument 2 of `Instance.new`")),
            "{diags:?}"
        );
    }

    #[test]
    fn ignores_instance_new_without_parent_arg() {
        let diags = run_rule_roblox(&Deprecated, "local p = Instance.new('Part')");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_deprecated_enum_item() {
        let diags = run_rule_roblox(&Deprecated, "local a = Enum.AlignType.Parallel");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`Enum.AlignType.Parallel` is deprecated"), "{diags:?}");
    }

    #[test]
    fn ignores_deprecated_enum_item_on_shadowed_root() {
        let diags =
            run_rule_roblox(&Deprecated, "local Enum = {}\nlocal a = Enum.AlignType.Parallel");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_deprecated_collectgarbage_option() {
        // Per-constant deprecation from the 5.4 GC rework.
        let diags = run_rule(&Deprecated, "collectgarbage('setpause', 100)", LuaVersion::Lua54);
        assert!(
            diags.iter().any(|diag| diag.message.contains("`setpause` is a deprecated argument")),
            "{diags:?}"
        );
    }

    #[test]
    fn bug_fix_keeps_math_pow_argument_grouping() {
        let after = run_fix("local r = math.pow(a + 1, b)", LuaVersion::Lua54).map(|fix| fix.1);
        assert_ne!(
            after.as_deref(),
            Some("local r = (a + 1 ^ b)"),
            "the fix shouldn't regroup the base as `1 ^ b`"
        );
    }

    #[test]
    fn bug_fix_keeps_getn_argument_grouping() {
        let after = run_fix("local r = table.getn(a or b)", LuaVersion::Lua54).map(|fix| fix.1);
        assert_ne!(
            after.as_deref(),
            Some("local r = #a or b"),
            "the fix should take the length of the whole argument"
        );
    }

    #[test]
    fn bug_fix_keeps_getn_grouping_in_power() {
        let after = run_fix("local r = table.getn(t) ^ 2", LuaVersion::Lua54).map(|fix| fix.1);
        assert_ne!(
            after.as_deref(),
            Some("local r = #t ^ 2"),
            "`#t ^ 2` takes the length of `t ^ 2`, not the square of the length"
        );
    }

    #[test]
    fn bug_fix_keeps_extra_unpack_arguments() {
        let after = run_fix("local a, b = unpack(t, 2, 3)", LuaVersion::Lua54).map(|fix| fix.1);
        assert_ne!(
            after.as_deref(),
            Some("local a, b = table.unpack(t)"),
            "the fix shouldn't drop the range arguments"
        );
    }

    #[test]
    fn bug_no_fix_when_template_arguments_are_missing() {
        let diags = run_rule(&Deprecated, "local r = math.pow(f())", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "`(f() ^ )` doesn't parse, so no fix applies: {diags:?}");
    }

    #[test]
    fn bug_no_fix_for_bit32_bnot_on_lua53() {
        let after = run_fix("local x = bit32.bnot(0)", LuaVersion::Lua53).map(|fix| fix.1);
        assert_ne!(
            after.as_deref(),
            Some("local x = (~0)"),
            "`bit32.bnot(0)` is 4294967295 and `~0` is -1"
        );
    }
}
