use luck_ast::{Expression, FunctionArgs, FunctionCall, Literal, is_multi_value};
use luck_semantic::SemanticAnalysis;
use luck_token::{Span, Token, short_string_body};

use crate::call::dotted_callee;
use crate::diagnostic::Finding;
use crate::format_pattern::{
    PatternError, count_pattern_captures, validate_format, validate_gsub_replacement,
    validate_lua_pattern, validate_os_date, validate_pack_format,
};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallPosition, LintContext, NodeRule, impl_rule};

/// Validates a literal format string, Lua pattern, or pack format passed to
/// a `string` function, and checks the argument count where one applies.
///
/// Both `string.format("...", ...)` and `("..."):format(...)` are checked.
/// So are literal `string.gsub` replacements and `os.date` formats.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FormatString;

impl_rule!(
    FormatString,
    "format_string",
    Correctness,
    Warning,
    "malformed format, pattern, pack, or date literal, or wrong argument count"
);

impl NodeRule for FormatString {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr])
    }
    fn on_call(
        &self,
        call: &FunctionCall,
        _position: CallPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        findings.extend(check_dsl_literal(call, context.semantic));
        findings.extend(check_gsub_replacement(call, context.semantic));
    }
}

/// Which validator applies to a stdlib call's literal.
#[derive(Debug, Clone, Copy)]
enum DslKind {
    /// `string.format`, with `%`-style printf specifiers.
    Format,
    /// `string.match`, `gmatch`, `find`, `gsub`, in the Lua pattern grammar.
    LuaPattern,
    /// `string.pack`, `packsize`, and `unpack` formats.
    Pack,
    /// `os.date`, with strftime-style specifiers.
    OsDate,
}

impl DslKind {
    /// The DSL of the `string` function `name`. `os.date` has no method
    /// form, so it never comes from here.
    fn of_string_function(name: &str) -> Option<Self> {
        match name {
            "format" => Some(Self::Format),
            "match" | "gmatch" | "find" | "gsub" => Some(Self::LuaPattern),
            "pack" | "packsize" | "unpack" => Some(Self::Pack),
            _ => None,
        }
    }

    /// Where the literal sits among a dotted call's arguments. A pattern
    /// follows the string it searches.
    const fn literal_index(self) -> usize {
        match self {
            Self::LuaPattern => 1,
            Self::Format | Self::Pack | Self::OsDate => 0,
        }
    }
}

/// Validates the literal of a dotted `string.*` or `os.date` call, or the
/// literal receiver of the method form `("..."):format(...)`.
fn check_dsl_literal(call: &FunctionCall, semantic: &SemanticAnalysis) -> Option<Finding> {
    // `trailing_args` are the arguments after the literal, or `None` when
    // string or table call syntax leaves no argument list to count.
    let (dsl, literal, trailing_args) = if let Some(method) = &call.method {
        let dsl = method.identifier().and_then(DslKind::of_string_function)?;
        let Expression::StringLiteral(receiver) = unwrap_parentheses(&call.callee) else {
            return None;
        };
        // The receiver isn't in the argument list, so every argument counts.
        let all_args = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => Some(args.items.as_slice()),
            FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => None,
        };
        (dsl, receiver, all_args)
    } else {
        let dsl = dotted_callee_dsl(call, semantic)?;
        match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                let literal_index = dsl.literal_index();
                let Some(Expression::StringLiteral(literal)) = args.get(literal_index) else {
                    return None;
                };
                (dsl, literal, args.items.get(literal_index.saturating_add(1)..))
            }
            FunctionArgs::StringLiteral(literal) if dsl.literal_index() == 0 => {
                (dsl, literal, None)
            }
            FunctionArgs::StringLiteral(_) | FunctionArgs::TableConstructor(_) => return None,
        }
    };

    // Escapes stay undecoded, and long-bracket strings go unchecked.
    let body = short_string_body(&literal.text)?;
    // The argument count each specifier or value option asks for. Pattern
    // functions and `os.date` take arguments the literal doesn't determine.
    let (noun, expected_arg_count) = match dsl {
        DslKind::Format => ("format", validate_format(body).map(Some)),
        DslKind::LuaPattern => ("pattern", validate_lua_pattern(body).map(|_| None)),
        DslKind::Pack => ("pack format", validate_pack_format(body).map(Some)),
        DslKind::OsDate => ("date format", validate_os_date(body).map(|()| None)),
    };
    match expected_arg_count {
        Ok(Some(expected)) => check_arg_count(trailing_args?, literal.span, expected),
        Ok(None) => None,
        Err(error) => Some(literal_error(error, literal, noun)),
    }
}

/// The DSL of a `string.*` or `os.date` callee whose root is a global, not a
/// shadowing local.
fn dotted_callee_dsl(call: &FunctionCall, semantic: &SemanticAnalysis) -> Option<DslKind> {
    let (root, field) = dotted_callee(call)?;
    let dsl = match (root.identifier()?, field) {
        ("string", _) => DslKind::of_string_function(field)?,
        ("os", "date") => DslKind::OsDate,
        _ => return None,
    };
    is_global(root, semantic).then_some(dsl)
}

/// Whether the name `token` reads a global. A shadowed root
/// (`local string = ...`) is a user value.
fn is_global(token: &Token, semantic: &SemanticAnalysis) -> bool {
    token.identifier().is_some_and(|name| !semantic.resolves_to_local(name, token.span))
}

/// Walks past any parens wrapping an expression, as in `("x"):format(...)`.
fn unwrap_parentheses(expr: &Expression) -> &Expression {
    let mut current = expr;
    while let Expression::Parenthesized(parenthesized) = current {
        current = &parenthesized.expr;
    }
    current
}

/// Flags a call whose argument count after the literal differs from
/// `expected`.
fn check_arg_count(args: &[Expression], literal_span: Span, expected: usize) -> Option<Finding> {
    // A `...` or call could expand to any number of values.
    if args.iter().any(is_multi_value) {
        return None;
    }
    let given = args.len();
    (given != expected).then(|| {
        Finding::new(
            format!("wrong argument count, format takes {expected}, got {given}"),
            literal_span,
        )
    })
}

/// Validates a literal `gsub` replacement string.
///
/// Capture indices are only checked when the pattern is a literal too.
fn check_gsub_replacement(call: &FunctionCall, semantic: &SemanticAnalysis) -> Option<Finding> {
    let pattern_index: usize = if let Some(method) = &call.method {
        if method.identifier() != Some("gsub") {
            return None;
        }
        0
    } else {
        let Some((root, "gsub")) = dotted_callee(call) else {
            return None;
        };
        if root.identifier() != Some("string") || !is_global(root, semantic) {
            return None;
        }
        1
    };
    let FunctionArgs::Parenthesized { args, .. } = &call.args else {
        return None;
    };
    let Some(Expression::StringLiteral(replacement)) = args.get(pattern_index.saturating_add(1))
    else {
        return None;
    };
    let replacement_body = short_string_body(&replacement.text)?;
    let capture_count = if let Some(Expression::StringLiteral(pattern)) = args.get(pattern_index) {
        short_string_body(&pattern.text).map(count_pattern_captures)
    } else {
        None
    };
    validate_gsub_replacement(replacement_body, capture_count)
        .err()
        .map(|error| literal_error(error, replacement, "replacement"))
}

/// Reports `error` at its offset in the body of the quoted `literal`.
fn literal_error(error: PatternError, literal: &Literal, noun: &str) -> Finding {
    let offset = error.offset();
    Finding::new(
        format!("invalid {noun}: {error}"),
        Span::from_indices(offset, offset.saturating_add(1))
            .shifted(literal.span.start.saturating_add(1)),
    )
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&FormatString, source, LuaVersion::Lua54)
    }

    #[test]
    fn ignores_correct_arg_count() {
        let diags = run("string.format(\"%d\", 1)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_too_few_args() {
        let diags = run("string.format(\"%q\")");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("format takes 1, got 0"), "msg: {}", diags[0].message);
    }

    #[test]
    fn flags_unknown_specifier() {
        let diags = run("string.format(\"%z\", 1)");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("unknown specifier"), "msg: {}", diags[0].message);
    }

    #[test]
    fn flags_pattern_unterminated_set() {
        let diags = run("string.match(\"abc\", \"[abc\")");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("unterminated set"), "msg: {}", diags[0].message);
    }

    #[test]
    fn ignores_valid_pack_format() {
        let diags = run("string.pack(\">i4\", 1)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_invalid_pack_option() {
        let diags = run("string.pack(\"Q\", 1)");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("invalid pack option"), "msg: {}", diags[0].message);
    }

    #[test]
    fn ignores_method_form_correct_format() {
        // The receiver is the format string, so `1` is the one argument
        // that `%d` needs.
        let diags = run("(\"%d\"):format(1)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_method_form_bad_format() {
        let diags = run("(\"%z\"):format(1)");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_nonliteral_format() {
        let diags = run("local fmt = \"%d\"\nstring.format(fmt, 1)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_vararg_args() {
        let diags = run("local function f(...) return string.format(\"%d %d\", ...) end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_function_call_args() {
        // `f()` can return any number of values.
        let diags = run("local function f() end\nstring.format(\"%d %d\", f())");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_long_bracket_strings() {
        let diags = run("string.format([[%z]], 1)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_gsub_bad_replacement() {
        let diags = run("string.gsub(\"x\", \"y\", \"%q\")");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("replacement"), "got: {diags:?}");
    }

    #[test]
    fn flags_gsub_unfinished_replacement() {
        let diags = run("string.gsub(\"x\", \"y\", \"a%\")");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_gsub_capture_out_of_range() {
        let diags = run("string.gsub(\"x\", \"(a)(b)\", \"%3\")");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("capture index"), "got: {diags:?}");
    }

    #[test]
    fn flags_gsub_method_form_replacement() {
        let diags = run("(\"x\"):gsub(\"(a)\", \"%2\")");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_gsub_valid_replacement() {
        // `%%` escapes, `%0` is the whole match, and `%1` is the first
        // capture, or the whole match when the pattern has none.
        for source in
            ["string.gsub(\"x\", \"(a)\", \"%%%0%1\")", "string.gsub(\"x\", \"a\", \"%1\")"]
        {
            let diags = run(source);
            assert!(diags.is_empty(), "got: {diags:?}");
        }
    }

    #[test]
    fn ignores_gsub_nonliteral_pattern_index() {
        // An unknown capture count disables the index check.
        let diags = run("local p = \"(a)\"\nstring.gsub(\"x\", p, \"%9\")");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_os_date_bad_specifier() {
        let diags = run("os.date(\"%Q\")");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("date format"), "got: {diags:?}");
    }

    #[test]
    fn flags_os_date_unfinished() {
        let diags = run("os.date(\"abc%\")");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_os_date_valid() {
        for format in ["\"%Y-%m-%d %H:%M:%S\"", "\"*t\"", "\"!*t\"", "\"100%%\""] {
            let diags = run(&format!("os.date({format})"));
            assert!(diags.is_empty(), "format {format}: {diags:?}");
        }
    }

    #[test]
    fn ignores_shadowed_os() {
        let diags = run("local os = {}\nos.date(\"%Q\")");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn bug_ignores_luau_star_format_specifier() {
        let diags = run_rule(&FormatString, "local s = string.format(\"%*\", 1)", LuaVersion::Luau);
        assert!(diags.is_empty(), "Luau accepts the `%*` conversion: {diags:?}");
    }

    #[test]
    fn bug_ignores_percent_p_format_on_lua54() {
        let diags = run("local s = string.format(\"%p\", {})");
        assert!(diags.is_empty(), "Lua 5.4 accepts the `%p` conversion: {diags:?}");
    }

    #[test]
    fn bug_flags_percent_a_format_on_lua51() {
        let diags =
            run_rule(&FormatString, "local s = string.format(\"%a\", 1)", LuaVersion::Lua51);
        assert_eq!(diags.len(), 1, "Lua 5.1 rejects the `%a` conversion: {diags:?}");
    }

    #[test]
    fn bug_ignores_repeated_format_flags() {
        let diags = run("local s = string.format(\"%--d\", 1)");
        assert!(diags.is_empty(), "every dialect accepts a repeated format flag: {diags:?}");
    }

    #[test]
    fn bug_flags_modifier_on_percent_q_on_lua54() {
        let diags = run("local s = string.format(\"%5q\", \"a\")");
        assert_eq!(diags.len(), 1, "Lua 5.4 rejects a modifier on `%q`: {diags:?}");
    }

    #[test]
    fn bug_flags_alternate_flag_on_percent_d_on_lua54() {
        let diags = run("local s = string.format(\"%#d\", 1)");
        assert_eq!(diags.len(), 1, "Lua 5.4 rejects the `#` flag on `%d`: {diags:?}");
    }

    #[test]
    fn bug_ignores_plain_find_special_characters() {
        let diags = run("local i = string.find(s, \"[\", 1, true)");
        assert!(diags.is_empty(), "a plain find doesn't read a pattern: {diags:?}");
    }

    #[test]
    fn bug_ignores_method_match_subject_literal() {
        let diags = run("local m = (\"[abc\"):match(\"b\")");
        assert!(diags.is_empty(), "the receiver of `:match` is the subject: {diags:?}");
    }

    #[test]
    fn bug_flags_method_match_bad_pattern() {
        let diags = run("local m = (\"abc\"):match(\"[\")");
        assert_eq!(diags.len(), 1, "the argument of `:match` is the pattern: {diags:?}");
    }

    #[test]
    fn bug_ignores_set_with_leading_close_bracket() {
        let diags = run("local i = string.find(s, \"[]]\")");
        assert!(diags.is_empty(), "a `]` first in a set is a member: {diags:?}");
    }

    #[test]
    fn bug_flags_back_reference_without_capture() {
        let diags = run("local i = string.find(s, \"%1\")");
        assert_eq!(diags.len(), 1, "`%1` with no capture raises an error: {diags:?}");
    }

    #[test]
    fn bug_ignores_packsize_value_options() {
        let diags = run("local n = string.packsize(\"i4\")");
        assert!(diags.is_empty(), "`string.packsize` takes only the format: {diags:?}");
    }

    #[test]
    fn bug_ignores_unpack_of_multiple_values() {
        let diags = run("local a, b = string.unpack(\"i4 i4\", s)");
        assert!(diags.is_empty(), "`string.unpack` takes the data, not one value each: {diags:?}");
    }

    #[test]
    fn bug_flags_pack_integer_size_out_of_limits() {
        let diags = run("local p = string.pack(\"i17\", 1)");
        assert_eq!(diags.len(), 1, "integer sizes above 16 raise an error: {diags:?}");
    }

    #[test]
    fn bug_format_error_span_covers_whole_character() {
        let source = "local s = string.format(\"%\u{e9}\", 1)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            diags[0].span.text(source),
            "\u{e9}",
            "the span should cover the whole character: {diags:?}"
        );
    }

    #[test]
    fn bug_format_error_message_does_not_garble_character() {
        let diags = run("local s = string.format(\"%\u{e9}\", 1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            !diags[0].message.contains('\u{c3}'),
            "the lead byte shouldn't print as a Latin-1 character: {diags:?}"
        );
    }
}
