use luck_allocator::Allocator;
use luck_ast::{
    AstKind, AstType, Call, Expression, GetSpan as _, Statement, Suffix, SuffixedExpression,
};
use luck_diagnostics::Tag;
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::{
    ResolvedCallee, SemanticAnalysis, StdlibEntry, StdlibFunction, expand_replace_template,
};
use luck_token::{index_to_offset, is_lua_whitespace, offset_to_index};

use crate::call::{CallArgs, calls_in, string_literal_value};
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::fix::{Fix, FixKind, RuleFixer};
use crate::rule::{RuleRunner, impl_rule};

/// A replacement the catalog marks exact is a fix, and any other one a
/// suggestion.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Deprecated;

impl_rule!(
    Deprecated,
    "deprecated",
    Correctness,
    Warning,
    "use of a deprecated standard library function, argument, or value",
    Fix | Suggestion
);

impl RuleRunner for Deprecated {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[AstType::SuffixedExpression])
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        // A call statement's call is its chain's last.
        let statement_call =
            context.is_statement(node).then(|| chain.suffixes.len().saturating_sub(1));
        for (index, call) in calls_in(chain) {
            if let Some(callee) = context.callee(node, index) {
                check_call(call, callee, statement_call == Some(index), context);
            }
        }
        check_field_accesses(node, chain, context);
    }
}

fn check_call(
    call: Call<'_>,
    callee: ResolvedCallee<'_>,
    is_statement: bool,
    context: &LintContext<'_>,
) {
    let entry = callee.entry();

    if let Some(deprecation) = entry.deprecation() {
        let name = callee.name();
        let finding = Finding::new(format!("`{name}` is deprecated"), call.span())
            .with_tag(Tag::Deprecated)
            .with_help(deprecation.message.to_owned());
        context.diagnostic(match deprecation.replacement {
            Some(replacement) => {
                let kind = if replacement.is_exact { FixKind::Fix } else { FixKind::Suggestion };
                finding.with_fix(context, kind, |fixer| {
                    replacement_fix(
                        fixer,
                        call,
                        entry,
                        is_statement,
                        &name,
                        replacement.template,
                        context,
                    )
                })
            }
            None => finding,
        });
        return;
    }

    if let StdlibEntry::Function(function) = entry {
        for finding in deprecated_constants(call, &callee, function)
            .chain(deprecated_params(call, &callee, function))
        {
            context.diagnostic(finding);
        }
    }
}

/// Flags each argument in a position that every signature matching the
/// call's arity marks deprecated, like the `parent` of `Instance.new`.
///
/// Any expression counts, since passing that argument at all is the problem.
fn deprecated_params(
    call: Call<'_>,
    callee: &ResolvedCallee<'_>,
    function: &StdlibFunction<'_>,
) -> impl Iterator<Item = Finding> {
    let args = CallArgs::new(call.args);
    let arg_count = args.len();
    args.enumerate().filter_map(move |(index, arg)| {
        let mut deprecations = function
            .matching_signatures(arg_count)
            .map(|signature| signature.params.get(index)?.deprecated.as_ref());
        // The first signature's deprecation speaks for all of them.
        let deprecation = deprecations.next().flatten()?;
        deprecations.all(|later| later.is_some()).then(|| {
            let arg_number = index.saturating_add(1);
            Finding::new(
                format!("argument {arg_number} of `{}` is deprecated", callee.name()),
                arg.span(),
            )
            .with_tag(Tag::Deprecated)
            .with_help(deprecation.message.to_owned())
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
    function: &StdlibFunction<'_>,
) -> impl Iterator<Item = Finding> {
    let args = CallArgs::new(call.args);
    let arg_count = args.len();
    args.enumerate().filter_map(move |(index, arg)| {
        let (value, span) = string_literal_value(arg)?;
        // Each signature's first constant spelled `value` counts, and the
        // first of those that's deprecated decides.
        let deprecation = function
            .allowed_constants(index, arg_count)?
            .filter_map(|values| values.iter().find(|constant| constant.value == value))
            .find_map(|constant| constant.deprecated.as_ref())?;
        Some(
            Finding::new(
                format!("`{value}` is a deprecated argument of `{}`", callee.name()),
                span,
            )
            .with_tag(Tag::Deprecated)
            .with_help(deprecation.message.to_owned()),
        )
    })
}

/// The replacement `template` filled in with the call's argument text, or
/// `None` when the result can't stand where the call does.
fn replacement_fix(
    fixer: RuleFixer,
    call: Call<'_>,
    entry: &StdlibEntry<'_>,
    is_statement: bool,
    name: &str,
    template: &str,
    context: &ContextHost<'_>,
) -> Option<Fix> {
    let StdlibEntry::Function(function) = entry else {
        return None;
    };
    let args = CallArgs::new(call.args);
    // A template spells the positional arguments, which a method's receiver
    // isn't among.
    if call.method.is_some() || !function.accepts_arg_count(args.len()) {
        return None;
    }
    let arg_texts: Vec<&[u8]> = args.map(|arg| arg.span().text(context.source)).collect();
    let expanded = expand_replace_template(template, &arg_texts);
    if starts_with_rebound_name(&expanded, call.span().start, context.semantic) {
        return None;
    }
    let replacement = fit_to_position(expanded, call.span().end, is_statement, context)?;
    let message =
        format!("replace deprecated `{name}` with `{}`", String::from_utf8_lossy(&replacement));
    Some(fixer.replace(message, call.span(), replacement))
}

/// Whether the name `expanded` starts with may not be the host's global at
/// `offset`, because a local before it or a write in the chunk rebinds it.
fn starts_with_rebound_name(expanded: &[u8], offset: u32, semantic: &SemanticAnalysis<'_>) -> bool {
    let name_length = expanded
        .iter()
        .position(|&b| !b.is_ascii_alphanumeric() && b != b'_')
        .unwrap_or(expanded.len());
    let name = str::from_utf8(expanded.get(..name_length).unwrap_or_default()).unwrap_or_default();
    let tree = semantic.scoping();
    !name.is_empty()
        && (tree.nearest_local(name, offset).is_some()
            || tree.name_id(name).is_some_and(|id| tree.is_assigned_global(id)))
}

/// `expanded` as it must read where the call stood, a statement when
/// `is_statement`, or `None` if it doesn't parse there. Parsing inside a block keeps a leading `#` from reading as a
/// shebang line.
fn fit_to_position(
    expanded: Vec<u8>,
    call_end: u32,
    is_statement: bool,
    context: &ContextHost<'_>,
) -> Option<Vec<u8>> {
    let version = context.semantic.version();
    let allocator = Allocator::new();
    if is_statement {
        let statement = [&b"do "[..], &expanded, b" end"].concat();
        let parse = luck_parser::parse(&allocator, &statement, version);
        return parse.errors.is_empty().then_some(expanded);
    }
    let return_stmt = [&b"return "[..], &expanded].concat();
    let parse = luck_parser::parse(&allocator, &return_stmt, version);
    if !parse.errors.is_empty() {
        return None;
    }
    let is_unary = matches!(
        &*parse.chunk.block.stmts,
        [Statement::Return(returned)]
            if matches!(returned.exprs.first(), Some(Expression::UnaryOp(_)))
    );
    // A unary expression stops at an operator binding looser than it, but
    // not at `^` or a suffix, which would capture only its operand.
    let should_group = is_unary && is_followed_by_tighter_binding(call_end, context);
    Some(if should_group { [&b"("[..], &expanded, b")"].concat() } else { expanded })
}

/// Whether the next token after `offset` binds tighter than a unary operator:
/// `^`, or a suffix that continues the expression.
fn is_followed_by_tighter_binding(offset: u32, context: &ContextHost<'_>) -> bool {
    let bytes = context.source;
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

/// Flags each dotted read in the field run `chain` starts with that resolves
/// to a deprecated constant, property, or namespace.
///
/// That includes a deprecated Roblox enum item or type. Each read checks its
/// own entry, so a chain whose prefix is deprecated warns once. Functions are
/// skipped because `check_call` reports them at the call.
fn check_field_accesses(
    node: &AstNode<'_>,
    chain: &SuffixedExpression<'_>,
    context: &LintContext<'_>,
) {
    let semantic = context.semantic;
    let (Expression::Name(root), Suffix::Field(_)) = (&chain.prefix, chain.suffixes.first()) else {
        return;
    };
    if semantic.scoping().resolves_to_local(root) {
        return;
    }
    let stdlib = semantic.stdlib();
    let Some(mut entry) = stdlib.globals.get(root.text.as_str()) else {
        return;
    };
    // A target's last field is stored to, not read.
    let read_count = if context.is_assignment_target(node) {
        chain.suffixes.len().saturating_sub(1)
    } else {
        chain.suffixes.len()
    };
    let reads = chain.suffixes.get(..read_count).unwrap_or_default();
    for (read_index, suffix) in (1usize..).zip(reads) {
        let Suffix::Field(name) = suffix else {
            return;
        };
        // Nothing past an unknown path resolves either.
        let Some(member) = stdlib.child(entry, name.text.as_str()) else {
            return;
        };
        entry = member;
        if matches!(entry, StdlibEntry::Function(_)) {
            continue;
        }
        if let Some(deprecation) = entry.deprecation() {
            let path = chain.head(read_index).dotted_path().map(|path| path.to_string());
            context.diagnostic(
                Finding::new(format!("`{}` is deprecated", path.unwrap_or_default()), name.span)
                    .with_tag(Tag::Deprecated)
                    .with_help(deprecation.message.to_owned()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use core::slice;

    use luck_token::LuaVersion;

    use super::{Deprecated, Tag};
    use crate::diagnostic::Message;
    use crate::fix::FixKind;
    use crate::fixtures::{fix_and_reparse, run_rule, run_rule_roblox};

    fn run_fix(source: &str, version: LuaVersion) -> Option<(String, String)> {
        let diags = run_rule(&Deprecated, source, version);
        let diag = diags.into_iter().find(|diag| diag.fix.is_some())?;
        let applied = fix_and_reparse(source, slice::from_ref(&diag), version);
        Some((diag.fix?.message.into_owned(), applied))
    }

    fn fix_kind(diags: &[Message]) -> Option<FixKind> {
        diags.first()?.fix.as_ref().map(|fix| fix.kind)
    }

    #[test]
    fn flags_and_fixes_an_exact_replacement() {
        let diags = run_rule(&Deprecated, "unpack(t)", LuaVersion::Lua54);
        assert_eq!(fix_kind(&diags), Some(FixKind::Fix), "{diags:?}");
        assert_eq!(diags[0].tag, Some(Tag::Deprecated), "an editor strikes it through");
    }

    #[test]
    fn flags_and_suggests_a_replacement_that_changes_behavior() {
        let getn = run_rule(&Deprecated, "local n = table.getn(t)", LuaVersion::Lua54);
        assert_eq!(fix_kind(&getn), Some(FixKind::Suggestion), "`#` calls `__len`: {getn:?}");
        let wait = run_rule_roblox(&Deprecated, "wait(1)");
        assert_eq!(fix_kind(&wait), Some(FixKind::Suggestion), "{wait:?}");
    }

    #[test]
    fn flags_and_fixes_keeping_a_comment_inside_an_argument() {
        let (_, after) = run_fix("unpack(f(--[[why]] t))", LuaVersion::Lua54).unwrap();
        assert_eq!(after, "table.unpack(f(--[[why]] t))");
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
    fn flags_dead_service_name_without_parentheses() {
        let diags = run_rule_roblox(&Deprecated, "local p = game:GetService'PointsService'");
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
        let diags = run_rule_roblox(
            &Deprecated,
            "\
local Enum = {}
local a = Enum.AlignType.Parallel",
        );
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
    fn flags_deprecated_collectgarbage_option_without_parentheses() {
        let diags = run_rule(&Deprecated, "collectgarbage'setpause'", LuaVersion::Lua54);
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
        let diags = run_rule(
            &Deprecated,
            "\
local a = 1
table.getn(t)",
            LuaVersion::Lua54,
        );
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
        let diags = run_rule(
            &Deprecated,
            "\
local table = {}
local a = unpack(t)",
            LuaVersion::Lua54,
        );
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
