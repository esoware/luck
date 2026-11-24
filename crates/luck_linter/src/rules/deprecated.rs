use luck_allocator::Allocator;
use luck_ast::{
    AstKind, AstType, Call, Expression, GetSpan as _, Statement, Suffix, SuffixedExpression,
};
use luck_diagnostics::Tag;
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::{
    ResolvedCallee, SemanticAnalysis, StdlibEntry, StdlibFunction, expand_replace_template,
};
use luck_token::{index_to_offset, is_lua_whitespace};

use crate::call::CallArgs;
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::fix::{Fix, FixKind, RuleFixer};
use crate::rule::{Rule, declare_lint_rule};

/// A replacement the catalog marks exact is a fix, and any other one a
/// suggestion.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Deprecated;

declare_lint_rule!(
    Deprecated,
    Correctness,
    Warning,
    "use of a deprecated standard library function, argument, or value",
    fix = Fix | Suggestion
);

impl Rule for Deprecated {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        let statement_call = context.statement_call(node);
        for (index, call) in chain.calls() {
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
    context: &LintContext<'_, '_>,
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
            let arg_number = index + 1;
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
        let (value, span) = arg.string_literal_value()?;
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
    let name = str::from_utf8(&expanded[..name_length]).expect("a name's bytes are ASCII");
    let tree = semantic.scoping();
    !name.is_empty()
        && (tree.nearest_local(name, offset).is_some()
            || tree.name_id(name).is_some_and(|id| tree.is_assigned_global(id)))
}

/// `expanded` as it must read where the call stood, a statement when
/// `is_statement`, or `None` if it doesn't parse there.
///
/// Parsing inside a block keeps a leading `#` from reading as a shebang
/// line.
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
    let mut index = offset as usize;
    loop {
        index += bytes[index..].iter().take_while(|&&b| is_lua_whitespace(b)).count();
        let found = context
            .comments
            .binary_search_by_key(&index_to_offset(index), |comment| comment.span.start)
            .ok()
            .map(|position| &context.comments[position]);
        let Some(comment) = found else {
            break;
        };
        index = comment.span.end as usize;
    }
    match &bytes[index..] {
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
    context: &LintContext<'_, '_>,
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
    for (read_index, suffix) in (1..).zip(context.read_suffixes(node)) {
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
            let path = chain
                .head(read_index)
                .dotted_path()
                .expect("every suffix read so far is a field of a name");
            context.diagnostic(
                Finding::new(format!("`{path}` is deprecated"), name.span)
                    .with_tag(Tag::Deprecated)
                    .with_help(deprecation.message.to_owned()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        // The type of `obj` is unknown, so `getn` doesn't resolve to a stdlib
        // entry.
        ("obj:getn()", LuaTarget::Lua54),
        ("local p = game:GetService('Players')", LuaTarget::LuauRoblox),
        ("local p = Instance.new('Part')", LuaTarget::LuauRoblox),
        (
            "\
local Enum = {}
local a = Enum.AlignType.Parallel",
            LuaTarget::LuauRoblox,
        ),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("unpack(t)", LuaTarget::Lua54),
        // Only a suggestion, since `#` calls `__len`.
        ("local n = table.getn(t)", LuaTarget::Lua54),
        ("wait(1)", LuaTarget::LuauRoblox),
        ("unpack(f(--[[why]] t))", LuaTarget::Lua54),
        ("loadstring('x')", LuaTarget::Lua54),
        ("string.gfind(s, p)", LuaTarget::Lua54),
        ("local r = math.pow(a, b)", LuaTarget::Lua54),
        // `(a ^ b)` can't stand alone as a statement, so there's no fix.
        ("math.pow(a, b)", LuaTarget::Lua54),
        ("math.log10(x)", LuaTarget::Lua54),
        // A string literal has the `string` shape, so `:gfind` warns like
        // `string.gfind`.
        ("local it = ('x'):gfind('a')", LuaTarget::Lua51),
        ("local p = game:GetService('PointsService')", LuaTarget::LuauRoblox),
        ("local p = game:GetService'PointsService'", LuaTarget::LuauRoblox),
        ("local p = Instance.new('Part', workspace)", LuaTarget::LuauRoblox),
        ("local a = Enum.AlignType.Parallel", LuaTarget::LuauRoblox),
        // Per-constant deprecation from the 5.4 GC rework.
        ("collectgarbage('setpause', 100)", LuaTarget::Lua54),
        ("collectgarbage'setpause'", LuaTarget::Lua54),
        // The fix shouldn't regroup the base as `1 ^ b`.
        ("local r = math.pow(a + 1, b)", LuaTarget::Lua54),
        // The fix should take the length of the whole argument.
        ("local r = table.getn(a or b)", LuaTarget::Lua54),
        // `#t ^ 2` takes the length of `t ^ 2`, not the square of the length.
        ("local r = table.getn(t) ^ 2", LuaTarget::Lua54),
        // The fix shouldn't drop the range arguments.
        ("local a, b = unpack(t, 2, 3)", LuaTarget::Lua54),
        // `(f() ^ )` doesn't parse, so no fix applies.
        ("local r = math.pow(f())", LuaTarget::Lua54),
        // `bit32.bnot(0)` is 4294967295 and `~0` is -1.
        ("local x = bit32.bnot(0)", LuaTarget::Lua53),
        // `#t` can't stand as a statement.
        (
            "\
local a = 1
table.getn(t)",
            LuaTarget::Lua54,
        ),
        ("local r = table.getn(t).x", LuaTarget::Lua54),
        ("local r = table.getn(t) .. 'x'", LuaTarget::Lua54),
        ("local r = math.pow(a --[[why]], b)", LuaTarget::Lua54),
        (
            "\
local table = {}
local a = unpack(t)",
            LuaTarget::Lua54,
        ),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::Deprecated, PASS, FAIL).test_and_snapshot();
    }
}
