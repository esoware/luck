use luck_ast::{Call, Expression, FunctionArgs, LastStatement, Suffix, SuffixedExpression};
use luck_semantic::{
    ResolvedCallee, SemanticAnalysis, StdlibDeprecation, StdlibEntry, StdlibFunction,
    expand_replace_template,
};
use luck_token::{index_to_offset, is_lua_whitespace, offset_to_index};

use crate::call::{positional_args, string_literal_value};
use crate::diagnostic::Finding;
use crate::fix::{Fix, TextEdit, comments_within};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallPosition, CallSite, ChainPosition, LintContext, NodeRule, impl_rule};

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
            NodeType::Suffixed,
        ])
    }
    fn on_call(&self, site: &CallSite<'_>, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        check_call(site, context, findings);
    }
    fn on_chain(
        &self,
        chain: &SuffixedExpression,
        position: ChainPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        check_field_accesses(chain, position, context.semantic, findings);
    }
}

fn check_call(site: &CallSite<'_>, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
    let CallSite { call, position, callee: Some(callee) } = *site else {
        return;
    };
    let entry = callee.entry();

    if let Some(deprecation) = entry.deprecation() {
        let name = callee.name();
        let fix = replacement_fix(call, entry, position, &name, deprecation, context);
        findings.push(
            Finding::new(format!("`{name}` is deprecated"), call.span())
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
    call: Call<'_>,
    callee: &ResolvedCallee<'_>,
    function: &StdlibFunction,
) -> impl Iterator<Item = Finding> {
    let (args, arg_count) = positional_args(call.args);
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
    call: Call<'_>,
    callee: &ResolvedCallee<'_>,
    function: &StdlibFunction,
) -> impl Iterator<Item = Finding> {
    let (args, arg_count) = positional_args(call.args);
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
    call: Call<'_>,
    entry: &StdlibEntry,
    position: CallPosition,
    display_name: &str,
    deprecation: &StdlibDeprecation,
    context: &LintContext<'_>,
) -> Option<Fix> {
    let template = deprecation.replace_template.as_ref()?;
    let StdlibEntry::Function(function) = entry else {
        return None;
    };
    // A template spells the positional arguments, which a method's receiver
    // isn't among, and the argument text can't carry a comment between them.
    if call.method.is_some()
        || !function.accepts_arg_count(positional_args(call.args).1)
        || !comments_within(context.comments, call.span()).is_empty()
    {
        return None;
    }
    let expanded = expand_replace_template(template, &arg_texts(call.args, context.source));
    if starts_with_rebound_name(&expanded, call.span().start, context.semantic) {
        return None;
    }
    let replacement = fit_to_position(expanded, call.span().end, position, context)?;
    Some(Fix {
        description: format!("replace deprecated `{display_name}` with `{replacement}`"),
        edits: vec![TextEdit { span: call.span(), replacement }],
    })
}

/// Whether the name `expanded` starts with may not be the host's global at
/// `offset`, because a local before it or a write in the chunk rebinds it.
fn starts_with_rebound_name(expanded: &str, offset: u32, semantic: &SemanticAnalysis) -> bool {
    let name_length =
        expanded.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(expanded.len());
    let name = expanded.get(..name_length).unwrap_or_default();
    !name.is_empty()
        && (semantic.nearest_local(name, offset).is_some()
            || semantic.scope_tree.assigned_globals.contains(name))
}

/// `expanded` as it must read where the call stood, or `None` if it doesn't
/// parse there. Parsing inside a block keeps a leading `#` from reading as a
/// shebang line.
fn fit_to_position(
    expanded: String,
    call_end: u32,
    position: CallPosition,
    context: &LintContext<'_>,
) -> Option<String> {
    let version = context.semantic.version;
    if position == CallPosition::Statement {
        let parse = luck_parser::parse(format!("do {expanded} end"), version);
        return parse.errors.is_empty().then_some(expanded);
    }
    let parse = luck_parser::parse(format!("return {expanded}"), version);
    if !parse.errors.is_empty() {
        return None;
    }
    let is_unary = matches!(
        parse.block.last_stmt.as_deref(),
        Some(LastStatement::Return(returned))
            if matches!(returned.exprs.first(), Some(Expression::UnaryOp(_)))
    );
    // A unary expression stops at an operator binding looser than it, but
    // not at `^` or a suffix, which would capture only its operand.
    let should_group = is_unary && is_followed_by_tighter_binding(call_end, context);
    Some(if should_group { format!("({expanded})") } else { expanded })
}

/// Whether the next token after `offset` binds tighter than a unary operator:
/// `^`, or a suffix that continues the expression.
fn is_followed_by_tighter_binding(offset: u32, context: &LintContext<'_>) -> bool {
    let bytes = context.source.as_bytes();
    let mut index = offset_to_index(offset);
    loop {
        index = index.saturating_add(
            bytes
                .get(index..)
                .unwrap_or_default()
                .iter()
                .take_while(|&&b| is_lua_whitespace(b))
                .count(),
        );
        let found = context
            .comments
            .binary_search_by_key(&index_to_offset(index), |comment| comment.span.start)
            .ok()
            .and_then(|position| context.comments.get(position));
        let Some(comment) = found else {
            break;
        };
        index = offset_to_index(comment.span.end);
    }
    match bytes.get(index..).unwrap_or_default() {
        // `..` concatenates, which binds looser than a unary operator.
        [b'.', second, ..] => *second != b'.',
        [b'^' | b':' | b'[' | b'(' | b'{' | b'"' | b'\'', ..] => true,
        _ => false,
    }
}

fn arg_texts<'source>(function_args: &FunctionArgs, source: &'source str) -> Vec<&'source str> {
    match function_args {
        FunctionArgs::Parenthesized { args, .. } => {
            args.iter().map(|expr| expr.span().text(source)).collect()
        }
        FunctionArgs::StringLiteral(literal) => vec![literal.span.text(source)],
        FunctionArgs::TableConstructor(table) => vec![table.span.text(source)],
    }
}

/// Flags each dotted read in the field run `chain` starts with that resolves
/// to a deprecated constant, property, or namespace.
///
/// That includes a deprecated Roblox enum item or type. Each read checks its
/// own entry, so a chain whose prefix is deprecated warns once. Functions are
/// skipped because `check_call` reports them at the call.
fn check_field_accesses(
    chain: &SuffixedExpression,
    position: ChainPosition,
    semantic: &SemanticAnalysis,
    findings: &mut Vec<Finding>,
) {
    // A target's last field is stored to, not read.
    let read_count = match position {
        ChainPosition::Value => chain.suffixes.len(),
        ChainPosition::AssignmentTarget => chain.suffixes.len().saturating_sub(1),
    };
    let reads = chain.suffixes.get(..read_count).unwrap_or_default();
    let (Expression::Name(root), Some(Suffix::Field(_))) = (&chain.prefix, reads.first()) else {
        return;
    };
    let Some(root_name) = root.identifier() else {
        return;
    };
    let stdlib = semantic.stdlib();
    let Some(mut entry) = stdlib.globals.get(root_name) else {
        return;
    };
    for (read_index, suffix) in (1usize..).zip(reads) {
        let Suffix::Field(name) = suffix else {
            return;
        };
        // Nothing past an unknown path resolves either.
        let Some(member) = name.identifier().and_then(|segment| stdlib.child(entry, segment))
        else {
            return;
        };
        entry = member;
        if matches!(entry, StdlibEntry::Function(_)) {
            continue;
        }
        if let Some(deprecation) = entry.deprecation()
            && !semantic.resolves_to_local(root_name, root.span)
        {
            let path = chain.head(read_index).dotted_path().map(|path| path.segments.join("."));
            findings.push(
                Finding::new(format!("`{}` is deprecated", path.unwrap_or_default()), name.span)
                    .with_help(deprecation.message.as_str()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use core::slice;

    use luck_token::LuaVersion;

    use super::Deprecated;
    use crate::fixtures::{fix_and_reparse, run_rule, run_rule_roblox};

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
        let (description, after) = run_fix("local n = table.getn(t)", LuaVersion::Lua54).unwrap();
        assert_eq!(description, "replace deprecated `table.getn` with `#t`");
        assert_eq!(after, "local n = #t");
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
    fn flags_and_fixes_keeping_math_pow_argument_grouping() {
        let after = run_fix("local r = math.pow(a + 1, b)", LuaVersion::Lua54).map(|fix| fix.1);
        assert_ne!(
            after.as_deref(),
            Some("local r = (a + 1 ^ b)"),
            "the fix shouldn't regroup the base as `1 ^ b`"
        );
    }

    #[test]
    fn flags_and_fixes_keeping_getn_argument_grouping() {
        let after = run_fix("local r = table.getn(a or b)", LuaVersion::Lua54).map(|fix| fix.1);
        assert_ne!(
            after.as_deref(),
            Some("local r = #a or b"),
            "the fix should take the length of the whole argument"
        );
    }

    #[test]
    fn flags_and_fixes_keeping_getn_grouping_in_power() {
        let after = run_fix("local r = table.getn(t) ^ 2", LuaVersion::Lua54).map(|fix| fix.1);
        assert_ne!(
            after.as_deref(),
            Some("local r = #t ^ 2"),
            "`#t ^ 2` takes the length of `t ^ 2`, not the square of the length"
        );
    }

    #[test]
    fn flags_and_fixes_keeping_extra_unpack_arguments() {
        let after = run_fix("local a, b = unpack(t, 2, 3)", LuaVersion::Lua54).map(|fix| fix.1);
        assert_ne!(
            after.as_deref(),
            Some("local a, b = table.unpack(t)"),
            "the fix shouldn't drop the range arguments"
        );
    }

    #[test]
    fn flags_without_fix_when_template_arguments_are_missing() {
        let diags = run_rule(&Deprecated, "local r = math.pow(f())", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "`(f() ^ )` doesn't parse, so no fix applies: {diags:?}");
    }

    #[test]
    fn flags_without_fix_for_bit32_bnot_on_lua53() {
        let after = run_fix("local x = bit32.bnot(0)", LuaVersion::Lua53).map(|fix| fix.1);
        assert_ne!(
            after.as_deref(),
            Some("local x = (~0)"),
            "`bit32.bnot(0)` is 4294967295 and `~0` is -1"
        );
    }

    #[test]
    fn flags_without_fix_for_length_template_as_statement() {
        let diags = run_rule(&Deprecated, "local a = 1\ntable.getn(t)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "`#t` can't stand as a statement: {diags:?}");
    }

    #[test]
    fn flags_and_fixes_grouping_length_before_a_suffix() {
        let after = run_fix("local r = table.getn(t).x", LuaVersion::Lua54).map(|fix| fix.1);
        assert_eq!(after.as_deref(), Some("local r = (#t).x"));
    }

    #[test]
    fn flags_and_fixes_length_before_concatenation_without_grouping() {
        let after = run_fix("local r = table.getn(t) .. 'x'", LuaVersion::Lua54).map(|fix| fix.1);
        assert_eq!(after.as_deref(), Some("local r = #t .. 'x'"));
    }

    #[test]
    fn flags_without_fix_when_a_comment_sits_inside_the_call() {
        let diags = run_rule(&Deprecated, "local r = math.pow(a --[[why]], b)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "{diags:?}");
    }

    #[test]
    fn flags_without_fix_when_the_replacement_name_is_shadowed() {
        let diags =
            run_rule(&Deprecated, "local table = {}\nlocal a = unpack(t)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "{diags:?}");
    }

    #[test]
    fn flags_without_fix_for_a_method_call() {
        let diags = run_rule(&Deprecated, "local it = ('x'):gfind('a')", LuaVersion::Lua51);
        assert!(diags.iter().any(|diag| diag.message.contains("gfind")), "{diags:?}");
        assert!(diags.iter().all(|diag| diag.fix.is_none()), "{diags:?}");
    }
}
