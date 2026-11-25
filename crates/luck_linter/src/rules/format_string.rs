use luck_ast::{AstKind, AstType, Call, Expression, StringLiteral};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{Span, index_to_offset};
use luck_value::ValueContext;

use crate::call::{CallArg, CallArgs, dotted_callee};
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::format_pattern::{
    PatternError, validate_format, validate_gsub_replacement, validate_luau_pattern,
    validate_os_date, validate_pack_format,
};
use crate::rule::{Rule, declare_lint_rule};

/// Both `string.format("...", ...)` and `("..."):format(...)` are checked.
/// So are literal `string.gsub` replacements and `os.date` formats.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FormatString;

declare_lint_rule!(
    FormatString,
    Correctness,
    Warning,
    "malformed format, pattern, pack, or date literal, or wrong argument count"
);

impl Rule for FormatString {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

    /// Only a call that reaches the stdlib's function is checked, so a
    /// shadowed `string` isn't.
    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for (index, call) in chain.calls() {
            let Some(dsl) = DslKind::of_call(call) else {
                continue;
            };
            if context.callee(node, index).is_none() {
                continue;
            }
            if let Some(finding) = check_dsl_literal(call, dsl, context) {
                context.diagnostic(finding);
            }
            if dsl == DslKind::Gsub
                && let Some(finding) = check_gsub_replacement(call)
            {
                context.diagnostic(finding);
            }
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
            return Self::of_string_function(method.text.as_str());
        }
        let (root, field) = dotted_callee(call)?;
        match (root.text.as_str(), field) {
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

    /// Whether the literal is a Luau pattern, which follows the string it
    /// searches. Every other DSL's literal is the call's first operand.
    const fn is_pattern(self) -> bool {
        match self {
            Self::Match | Self::Find | Self::Gsub => true,
            Self::Format | Self::Pack | Self::PackSize | Self::Unpack | Self::OsDate => false,
        }
    }
}

/// The literal `dsl` reads from `call` and the arguments after it.
///
/// In a method call the receiver is the format, but the subject of a pattern.
fn dsl_literal(call: Call<'_>, dsl: DslKind) -> Option<(&StringLiteral<'_>, CallArgs<'_>)> {
    let is_method = call.method.is_some();
    let mut args = CallArgs::new(call.args);
    if is_method && !dsl.is_pattern() {
        let Some(Expression::StringLiteral(receiver)) = call.callee.as_expr() else {
            return None;
        };
        return Some((receiver, args));
    }
    let literal_index = usize::from(dsl.is_pattern() && !is_method);
    let literal = args.nth(literal_index)?.string_literal()?;
    Some((literal, args))
}

fn check_dsl_literal(call: Call<'_>, dsl: DslKind, context: &dyn ValueContext) -> Option<Finding> {
    let (literal, trailing_args) = dsl_literal(call, dsl)?;
    let body = literal.as_str()?;
    if dsl == DslKind::Find && is_plain_find(body, trailing_args.clone(), context) {
        return None;
    }
    // The argument count each specifier or value option asks for. Pattern
    // functions, `os.date`, and the other pack functions take arguments the
    // literal doesn't determine.
    let (noun, expected_arg_count) = match dsl {
        DslKind::Format => ("format", validate_format(body).map(Some)),
        DslKind::Match | DslKind::Find | DslKind::Gsub => {
            ("pattern", validate_luau_pattern(body).map(|_| None))
        }
        DslKind::Pack => ("pack format", validate_pack_format(body, false).map(Some)),
        DslKind::PackSize => ("pack format", validate_pack_format(body, true).map(|_| None)),
        DslKind::Unpack => ("pack format", validate_pack_format(body, false).map(|_| None)),
        DslKind::OsDate => ("date format", validate_os_date(body).map(|()| None)),
    };
    match expected_arg_count {
        Ok(Some(expected)) => check_arg_count(trailing_args, literal.span, expected),
        Ok(None) => None,
        Err(error) => Some(literal_error(error, literal, body, noun)),
    }
}

/// Whether `string.find` searches for `pattern` as plain text instead of
/// parsing it, going by `after_pattern`, the arguments after it.
///
/// `str_find_aux` also searches plainly when the pattern has no character
/// from `SPECIALS`, which is what lets a lone `)` through.
fn is_plain_find(
    pattern: &str,
    mut after_pattern: CallArgs<'_>,
    context: &dyn ValueContext,
) -> bool {
    let is_maybe_plain = match (after_pattern.next(), after_pattern.next()) {
        (None, _) => false,
        // A call or `...` here can expand over `init` and `plain`.
        (Some(init), None) => init.is_multi_value(),
        (Some(_), Some(plain)) => plain.to_boolean(context) != Some(false),
    };
    is_maybe_plain || !pattern.contains(['^', '$', '*', '+', '?', '.', '(', '[', '%', '-'])
}

fn check_arg_count(mut args: CallArgs<'_>, literal_span: Span, expected: usize) -> Option<Finding> {
    let given = args.len();
    // A `...` or call could expand to any number of values.
    if args.any(CallArg::is_multi_value) {
        return None;
    }
    (given != expected).then(|| {
        Finding::new(
            format!("wrong argument count, format takes {expected}, got {given}"),
            literal_span,
        )
    })
}

/// Capture indices are only checked when the pattern is a valid literal too.
fn check_gsub_replacement(call: Call<'_>) -> Option<Finding> {
    let pattern_index = usize::from(call.method.is_none());
    let mut args = CallArgs::new(call.args);
    let pattern = args.nth(pattern_index).and_then(CallArg::string_literal);
    let replacement = args.next()?.string_literal()?;
    let replacement_body = replacement.as_str()?;
    let capture_count =
        pattern.and_then(StringLiteral::as_str).and_then(|body| validate_luau_pattern(body).ok());
    validate_gsub_replacement(replacement_body, capture_count)
        .err()
        .map(|e| literal_error(e, replacement, replacement_body, "replacement"))
}

/// Reports `error` over the character at its offset in `body`, `literal`'s
/// value.
///
/// That's where the value is in the source when it's the literal's own text,
/// as it is with nothing to decode. Otherwise no offset in the value maps to
/// one in the source, so it's the whole literal.
fn literal_error(
    error: PatternError,
    literal: &StringLiteral<'_>,
    body: &str,
    noun: &str,
) -> Finding {
    let message = format!("invalid {noun}: {error}");
    let verbatim_start = literal.raw.and_then(|raw| {
        let raw_bytes = raw.as_bytes().as_ptr_range();
        let value = literal.value.as_bytes().as_ptr();
        raw_bytes.contains(&value).then(|| value.addr() - raw_bytes.start.addr())
    });
    let Some(value_start) = verbatim_start else {
        return Finding::new(message, literal.span);
    };
    let offset = error.offset();
    let width = body.get(offset..).and_then(|rest| rest.chars().next()).map_or(1, char::len_utf8);
    let start = literal.span.start + index_to_offset(value_start);
    Finding::new(message, Span::from_indices(offset, offset + width).shifted(start))
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        r#"string.format("%d", 1)"#,
        r#"local s = ("%s"):format"x""#,
        r#"local s = ("%s"):format{}"#,
        r#"string.pack(">i4", 1)"#,
        // The receiver is the format string, so `1` is the one argument that `%d`
        // needs.
        r#"("%d"):format(1)"#,
        "\
local fmt = \"%d\"
string.format(fmt, 1)",
        r#"local function f(...) return string.format("%d %d", ...) end"#,
        // `f()` can return any number of values.
        "\
local function f() end
string.format(\"%d %d\", f())",
        // `%%` escapes, `%0` is the whole match, and `%1` is the first capture, or
        // the whole match when the pattern has none.
        r#"string.gsub("x", "(a)", "%%%0%1")"#,
        r#"string.gsub("x", "a", "%1")"#,
        // An unknown capture count disables the index check.
        "\
local p = \"(a)\"
string.gsub(\"x\", p, \"%9\")",
        r#"os.date("%Y-%m-%d %H:%M:%S")"#,
        r#"os.date("*t")"#,
        r#"os.date("!*t")"#,
        r#"os.date("100%%")"#,
        // A trailing `%` is text.
        r#"os.date("abc%")"#,
        "\
local os = {}
os.date(\"%Q\")",
        // `%*` takes any value.
        r#"local s = string.format("%*", 1)"#,
        // `scanformat` takes a repeated flag, and any flag, width, or precision
        // before any conversion.
        r#"local s = string.format("%--d", 1)"#,
        r#"local s = string.format("%5q", "a")"#,
        r#"local s = string.format("%#d", 1)"#,
        // A plain find doesn't read a pattern.
        r#"local i = string.find(s, "[", 1, true)"#,
        // The receiver of `:match` is the subject.
        r#"local m = ("[abc"):match("b")"#,
        // `lstrlib.cpp` reads these as literals.
        r#"local s = string.gsub(x, "-", "_")"#,
        r#"local s = string.find(x, "+")"#,
        r#"local s = string.match(x, "^-?%d+$")"#,
        // A `]` first in a set is a member.
        r#"local i = string.find(s, "[]]")"#,
        // `string.packsize` takes only the format.
        r#"local n = string.packsize("i4")"#,
        // `string.unpack` takes the data, not one value each.
        r#"local a, b = string.unpack("i4 i4", s)"#,
        // A pattern with no special characters is plain.
        r#"local i = string.find(s, "a)")"#,
        r#"local i = string.find(s, "[", 1, plain)"#,
        // `obj` isn't known to be a string.
        r#"local i = obj:match("[")"#,
        // `\37` spells a `%` the raw body can't show.
        r#"local s = string.format("\37d", 1)"#,
        "\
string = {}
local i = string.match(\"x\", \"[\")",
    ];

    const FAIL: &[&str] = &[
        r#"string.format("%q")"#,
        r#"string.format"%d""#,
        r#"local s = ("%d %d"):format"x""#,
        r#"local s = ("%d %d"):format{}"#,
        r#"string.format("%z", 1)"#,
        r#"string.match("abc", "[abc")"#,
        r#"string.pack("Q", 1)"#,
        r#"("%z"):format(1)"#,
        "\
string.format([==[
%z]==], 1)",
        r#"string.format("\37z", 1)"#,
        r#"string.gsub("x", "y", "%q")"#,
        r#"string.gsub("x", "y", "a%")"#,
        r#"string.gsub("x", "(a)(b)", "%3")"#,
        r#"("x"):gsub("(a)", "%2")"#,
        r#"os.date("%Q")"#,
        // `os.date` takes only the C89 specifiers.
        r#"os.date("%F")"#,
        // There's no `%a` or `%p` conversion.
        r#"local s = string.format("%a", 1)"#,
        r#"local s = string.format("%p", {})"#,
        // The argument of `:match` is the pattern.
        r#"local m = ("abc"):match("[")"#,
        // `%1` with no capture raises an error.
        r#"local i = string.find(s, "%1")"#,
        // Integer sizes above 16 raise an error.
        r#"local p = string.pack("i17", 1)"#,
        // The span and the message take the whole character, not its first byte.
        "local s = string.format(\"%\u{e9}\", 1)",
        r#"local i = string.find(s, "[", 1, false)"#,
        "\
local s = \"abc\"
local i = s:match(\"[\")",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::FormatString, PASS, FAIL).test_and_snapshot();
    }
}
