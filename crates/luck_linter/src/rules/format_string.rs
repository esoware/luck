use luck_ast::{Call, Expression, FunctionArgs, Literal, is_multi_value, unwrap_parentheses};
use luck_token::{LuaVersion, Span, short_string_body};

use crate::call::dotted_callee;
use crate::diagnostic::Finding;
use crate::format_pattern::{
    PatternError, validate_format, validate_gsub_replacement, validate_lua_pattern,
    validate_os_date, validate_pack_format,
};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallSite, LintContext, NodeRule, impl_rule};

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
    fn on_call(&self, site: &CallSite<'_>, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let CallSite { call, callee: Some(_), .. } = *site else {
            return;
        };
        let semantic = context.semantic;
        let Some(dsl) = DslKind::of_call(call) else {
            return;
        };
        findings.extend(check_dsl_literal(call, dsl, semantic.version));
        if dsl == DslKind::Gsub {
            findings.extend(check_gsub_replacement(call, semantic.version));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DslKind {
    Format,
    Match,
    /// `string.find`, which may search its pattern as plain text.
    Find,
    /// `string.gsub`, which also takes a replacement.
    Gsub,
    /// `string.pack`, whose values the format counts.
    Pack,
    /// `string.packsize`, whose format can't hold a variable-length option.
    PackSize,
    Unpack,
    /// `os.date`, with strftime-style specifiers.
    OsDate,
}

impl DslKind {
    fn of_call(call: Call<'_>) -> Option<Self> {
        if let Some(method) = call.method {
            return Self::of_string_function(method.identifier()?);
        }
        let (root, field) = dotted_callee(call)?;
        match (root.identifier()?, field) {
            ("string", _) => Self::of_string_function(field),
            ("os", "date") => Some(Self::OsDate),
            _ => None,
        }
    }

    /// `os.date` has no method form, so it never comes from here.
    fn of_string_function(name: &str) -> Option<Self> {
        match name {
            "format" => Some(Self::Format),
            "match" | "gmatch" => Some(Self::Match),
            "find" => Some(Self::Find),
            "gsub" => Some(Self::Gsub),
            "pack" => Some(Self::Pack),
            "packsize" => Some(Self::PackSize),
            "unpack" => Some(Self::Unpack),
            _ => None,
        }
    }

    /// Whether the literal is a Lua pattern, which follows the string it
    /// searches. Every other DSL's literal is the call's first operand.
    const fn is_pattern(self) -> bool {
        match self {
            Self::Match | Self::Find | Self::Gsub => true,
            Self::Format | Self::Pack | Self::PackSize | Self::Unpack | Self::OsDate => false,
        }
    }
}

/// The literal `dsl` reads from `call` and the arguments after it, which are
/// `None` when string or table call syntax leaves no list to count.
///
/// In a method call the receiver is the format, but the subject of a pattern.
fn dsl_literal(call: Call<'_>, dsl: DslKind) -> Option<(&Literal, Option<&[Expression]>)> {
    let is_method = call.method.is_some();
    if is_method && !dsl.is_pattern() {
        let Some(Expression::StringLiteral(receiver)) =
            call.callee.as_expr().map(unwrap_parentheses)
        else {
            return None;
        };
        let all_args = match call.args {
            FunctionArgs::Parenthesized { args, .. } => Some(args.items.as_slice()),
            FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => None,
        };
        return Some((receiver, all_args));
    }
    let literal_index = usize::from(dsl.is_pattern() && !is_method);
    match call.args {
        FunctionArgs::Parenthesized { args, .. } => {
            let Some(Expression::StringLiteral(literal)) = args.get(literal_index) else {
                return None;
            };
            Some((literal, args.items.get(literal_index.saturating_add(1)..)))
        }
        FunctionArgs::StringLiteral(literal) if literal_index == 0 => Some((literal, None)),
        FunctionArgs::StringLiteral(_) | FunctionArgs::TableConstructor(_) => None,
    }
}

fn check_dsl_literal(call: Call<'_>, dsl: DslKind, version: LuaVersion) -> Option<Finding> {
    let (literal, trailing_args) = dsl_literal(call, dsl)?;
    let body = checkable_body(literal)?;
    if dsl == DslKind::Find && is_plain_find(body, trailing_args.unwrap_or_default()) {
        return None;
    }
    // The argument count each specifier or value option asks for. Pattern
    // functions, `os.date`, and the other pack functions take arguments the
    // literal doesn't determine.
    let (noun, expected_arg_count) = match dsl {
        DslKind::Format => ("format", validate_format(body, version).map(Some)),
        DslKind::Match | DslKind::Find | DslKind::Gsub => {
            ("pattern", validate_lua_pattern(body).map(|_| None))
        }
        DslKind::Pack => ("pack format", validate_pack_format(body, false).map(Some)),
        DslKind::PackSize => ("pack format", validate_pack_format(body, true).map(|_| None)),
        DslKind::Unpack => ("pack format", validate_pack_format(body, false).map(|_| None)),
        DslKind::OsDate => ("date format", validate_os_date(body, version).map(|()| None)),
    };
    match expected_arg_count {
        Ok(Some(expected)) => check_arg_count(trailing_args?, literal.span, expected),
        Ok(None) => None,
        Err(error) => Some(literal_error(error, literal, body, noun)),
    }
}

/// The body of `literal`, or `None` for one that can't be checked here: a
/// long-bracket string, or a body with a numeric escape, which could spell a
/// character the DSL reads, since escapes stay undecoded.
fn checkable_body(literal: &Literal) -> Option<&str> {
    let body = short_string_body(&literal.text)?;
    let mut bytes = body.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'\\'
            && bytes.next().is_some_and(|escaped| matches!(escaped, b'0'..=b'9' | b'x' | b'u'))
        {
            return None;
        }
    }
    Some(body)
}

/// Whether `string.find` searches for `pattern` as plain text instead of
/// parsing it, going by `after_pattern`, the arguments after it.
///
/// `str_find_aux` also searches plainly when the pattern has no character
/// from `SPECIALS`, which is what lets a lone `)` through.
fn is_plain_find(pattern: &str, after_pattern: &[Expression]) -> bool {
    let is_maybe_plain = match after_pattern {
        [] => false,
        // A call or `...` here can expand over `init` and `plain`.
        [init] => is_multi_value(init),
        [_, plain, ..] => !matches!(plain, Expression::Nil(_) | Expression::False(_)),
    };
    is_maybe_plain || !pattern.contains(['^', '$', '*', '+', '?', '.', '(', '[', '%', '-'])
}

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

/// Capture indices are only checked when the pattern is a valid literal too.
fn check_gsub_replacement(call: Call<'_>, version: LuaVersion) -> Option<Finding> {
    let pattern_index = usize::from(call.method.is_none());
    let FunctionArgs::Parenthesized { args, .. } = call.args else {
        return None;
    };
    let Some(Expression::StringLiteral(replacement)) = args.get(pattern_index.saturating_add(1))
    else {
        return None;
    };
    let replacement_body = checkable_body(replacement)?;
    let capture_count = if let Some(Expression::StringLiteral(pattern)) = args.get(pattern_index) {
        checkable_body(pattern).and_then(|body| validate_lua_pattern(body).ok())
    } else {
        None
    };
    validate_gsub_replacement(replacement_body, capture_count, version)
        .err()
        .map(|e| literal_error(e, replacement, replacement_body, "replacement"))
}

/// Reports `error` at its offset in the quoted `literal`, whose `body` the
/// offset counts into, over the whole character there.
fn literal_error(error: PatternError, literal: &Literal, body: &str, noun: &str) -> Finding {
    let offset = error.offset();
    let width = body.get(offset..).and_then(|rest| rest.chars().next()).map_or(1, char::len_utf8);
    Finding::new(
        format!("invalid {noun}: {error}"),
        Span::from_indices(offset, offset.saturating_add(width))
            .shifted(literal.span.start.saturating_add(1)),
    )
}

#[cfg(test)]
mod tests {
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
    fn ignores_luau_star_format_specifier() {
        let diags = run_rule(&FormatString, "local s = string.format(\"%*\", 1)", LuaVersion::Luau);
        assert!(diags.is_empty(), "Luau accepts the `%*` conversion: {diags:?}");
    }

    #[test]
    fn ignores_percent_p_format_on_lua54() {
        let diags = run("local s = string.format(\"%p\", {})");
        assert!(diags.is_empty(), "Lua 5.4 accepts the `%p` conversion: {diags:?}");
    }

    #[test]
    fn flags_percent_a_format_on_lua51() {
        let diags =
            run_rule(&FormatString, "local s = string.format(\"%a\", 1)", LuaVersion::Lua51);
        assert_eq!(diags.len(), 1, "Lua 5.1 rejects the `%a` conversion: {diags:?}");
    }

    #[test]
    fn ignores_repeated_format_flags() {
        let diags = run("local s = string.format(\"%--d\", 1)");
        assert!(diags.is_empty(), "every dialect accepts a repeated format flag: {diags:?}");
    }

    #[test]
    fn flags_modifier_on_percent_q_on_lua54() {
        let diags = run("local s = string.format(\"%5q\", \"a\")");
        assert_eq!(diags.len(), 1, "Lua 5.4 rejects a modifier on `%q`: {diags:?}");
    }

    #[test]
    fn flags_alternate_flag_on_percent_d_on_lua54() {
        let diags = run("local s = string.format(\"%#d\", 1)");
        assert_eq!(diags.len(), 1, "Lua 5.4 rejects the `#` flag on `%d`: {diags:?}");
    }

    #[test]
    fn ignores_plain_find_special_characters() {
        let diags = run("local i = string.find(s, \"[\", 1, true)");
        assert!(diags.is_empty(), "a plain find doesn't read a pattern: {diags:?}");
    }

    #[test]
    fn ignores_method_match_subject_literal() {
        let diags = run("local m = (\"[abc\"):match(\"b\")");
        assert!(diags.is_empty(), "the receiver of `:match` is the subject: {diags:?}");
    }

    #[test]
    fn flags_method_match_bad_pattern() {
        let diags = run("local m = (\"abc\"):match(\"[\")");
        assert_eq!(diags.len(), 1, "the argument of `:match` is the pattern: {diags:?}");
    }

    #[test]
    fn ignores_quantifier_with_nothing_to_repeat() {
        for source in [
            "local s = string.gsub(x, \"-\", \"_\")",
            "local s = string.find(x, \"+\")",
            "local s = string.match(x, \"^-?%d+$\")",
        ] {
            let diags = run(source);
            assert!(diags.is_empty(), "{source}: `lstrlib.c` reads these as literals: {diags:?}");
        }
    }

    #[test]
    fn ignores_set_with_leading_close_bracket() {
        let diags = run("local i = string.find(s, \"[]]\")");
        assert!(diags.is_empty(), "a `]` first in a set is a member: {diags:?}");
    }

    #[test]
    fn flags_back_reference_without_capture() {
        let diags = run("local i = string.find(s, \"%1\")");
        assert_eq!(diags.len(), 1, "`%1` with no capture raises an error: {diags:?}");
    }

    #[test]
    fn ignores_packsize_value_options() {
        let diags = run("local n = string.packsize(\"i4\")");
        assert!(diags.is_empty(), "`string.packsize` takes only the format: {diags:?}");
    }

    #[test]
    fn ignores_unpack_of_multiple_values() {
        let diags = run("local a, b = string.unpack(\"i4 i4\", s)");
        assert!(diags.is_empty(), "`string.unpack` takes the data, not one value each: {diags:?}");
    }

    #[test]
    fn flags_pack_integer_size_out_of_limits() {
        let diags = run("local p = string.pack(\"i17\", 1)");
        assert_eq!(diags.len(), 1, "integer sizes above 16 raise an error: {diags:?}");
    }

    #[test]
    fn flags_error_over_the_whole_character() {
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
    fn flags_error_without_garbling_the_character() {
        let diags = run("local s = string.format(\"%\u{e9}\", 1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            !diags[0].message.contains('\u{c3}'),
            "the lead byte shouldn't print as a Latin-1 character: {diags:?}"
        );
    }

    #[test]
    fn ignores_find_without_special_characters() {
        let diags = run("local i = string.find(s, \"a)\")");
        assert!(diags.is_empty(), "a pattern with no special characters is plain: {diags:?}");
    }

    #[test]
    fn flags_find_pattern_when_plain_is_false() {
        let diags = run("local i = string.find(s, \"[\", 1, false)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_find_with_unknown_plain_argument() {
        let diags = run("local i = string.find(s, \"[\", 1, plain)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_pattern_method_on_unknown_receiver() {
        let diags = run("local i = obj:match(\"[\")");
        assert!(diags.is_empty(), "`obj` isn't known to be a string: {diags:?}");
    }

    #[test]
    fn flags_pattern_method_on_string_local() {
        let diags = run("local s = \"abc\"\nlocal i = s:match(\"[\")");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_literal_with_numeric_escape() {
        let diags = run("local s = string.format(\"\\37d\", 1)");
        assert!(diags.is_empty(), "`\\37` spells a `%` the raw body can't show: {diags:?}");
    }

    #[test]
    fn flags_pack_on_lua53_but_not_lua51() {
        let source = "local p = string.pack(\"i17\", 1)";
        let on_53 = run_rule(&FormatString, source, LuaVersion::Lua53);
        assert_eq!(on_53.len(), 1, "{on_53:?}");
        let on_51 = run_rule(&FormatString, source, LuaVersion::Lua51);
        assert!(on_51.is_empty(), "Lua 5.1 has no `string.pack`: {on_51:?}");
    }

    #[test]
    fn ignores_rebound_string_global() {
        let diags = run("string = {}\nlocal i = string.match(\"x\", \"[\")");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
