use luck_ast::Expression;
use luck_ast::expr::{FunctionArgs, FunctionCall, Var};
use luck_ast::shared::Punctuated;
use luck_semantic::SemanticAnalysis;
use luck_token::{Span, TokenKind};

use crate::diagnostic::*;
use crate::format_pattern::{
    PatternError, validate_format, validate_lua_pattern, validate_pack_format,
};
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

/// Inspects calls to the `string.*` family that take a literal pattern
/// or format string and validates the literal up front. Both the dotted
/// form (`string.format("...", ...)`) and the method form
/// (`("literal"):format(...)`) are recognized. Literal `string.gsub`
/// replacement strings and `os.date` format strings are validated too.
pub struct FormatString;

impl Rule for FormatString {
    fn name(&self) -> &'static str {
        "format_string"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "string.format/match/pack/os.date literal is malformed or has wrong arg count"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

/// Which validator applies to a stdlib call.
#[derive(Debug, Clone, Copy)]
enum DslKind {
    /// `string.format` - `%`-style printf specifiers.
    Format,
    /// `string.match`, `gmatch`, `find`, `gsub` - Lua pattern grammar.
    LuaPattern,
    /// `string.pack`/`packsize`/`unpack` - fixed-width binary spec.
    Pack,
    /// `os.date` - strftime-style specifiers.
    OsDate,
}

/// The position of the literal argument and the expected non-vararg
/// extra-argument count behavior for arg-count checks.
#[derive(Debug, Clone, Copy)]
enum ArgCount {
    /// Format's spec-count equals the number of trailing args after the
    /// format string.
    ExactFormat,
    /// Pack: every value option consumes one trailing arg. Same shape
    /// as `ExactFormat`, just a different validator output.
    ExactPack,
    /// Pattern-based functions take auxiliary args (the haystack, plus
    /// possibly an init and a replacement) - arg-count check is skipped.
    None,
}

struct FormatChecker<'src, 'out> {
    source: &'src str,
    semantic: &'src SemanticAnalysis,
    out: &'out mut Vec<LintDiagnostic>,
}

impl<'src> FormatChecker<'src, '_> {
    fn check_call(&mut self, call: &FunctionCall) {
        let Some((dsl, arg_kind, literal_expr_idx)) = self.classify_callee(call) else {
            return;
        };

        // Long-bracket strings and non-literals bail.
        let (literal_token_span, args_punct) = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                let Some(expr) = nth_expr(args, literal_expr_idx) else {
                    return;
                };
                let Expression::StringLiteral(tok) = expr else {
                    return;
                };
                (tok.span, Some(args))
            }
            FunctionArgs::StringLiteral(tok) => {
                if literal_expr_idx != 0 {
                    return;
                }
                (tok.span, None)
            }
            FunctionArgs::TableConstructor(_) => return,
        };

        // Extract the raw pattern body - the substring between the
        // surrounding quotes/brackets - and the offset of the body in
        // source. Long-bracket strings are skipped because escape
        // handling makes raw-content mapping unreliable there.
        let Some((body, body_offset)) = pattern_body(self.source, literal_token_span) else {
            return;
        };

        match dsl {
            DslKind::Format => match validate_format(body) {
                Ok(specifier_count) => {
                    self.check_arg_count(arg_kind, args_punct, literal_token_span, specifier_count);
                }
                Err(err) => self.report_pattern_error(err, body_offset, "format"),
            },
            DslKind::LuaPattern => {
                if let Err(err) = validate_lua_pattern(body) {
                    self.report_pattern_error(err, body_offset, "pattern");
                }
            }
            DslKind::Pack => match validate_pack_format(body) {
                Ok(value_count) => {
                    self.check_arg_count(arg_kind, args_punct, literal_token_span, value_count);
                }
                Err(err) => self.report_pattern_error(err, body_offset, "pack format"),
            },
            DslKind::OsDate => {
                if let Err((message, offset)) = validate_os_date(body) {
                    self.report_literal_error(message, body_offset + offset as u32, "date format");
                }
            }
        }
    }

    /// Identify the callee shape and return (DSL kind, arg-count
    /// policy, index of the literal pattern within `args`).
    fn classify_callee(&self, call: &FunctionCall) -> Option<(DslKind, ArgCount, usize)> {
        // Method form `receiver:method(...)`. We only handle the case
        // where the receiver is a literal - `"hello":format(x)`. Any
        // variable receiver could be any kind of value, so we cannot
        // tell whether the method is `string.*` or a user method. For the
        // method form the "literal" is the receiver, not an entry in
        // `call.args`; `check_method_literal_receiver` handles that shape,
        // so this dotted-callee resolver bows out.
        if call.method.is_some() {
            return None;
        }

        let (root, field) = self.global_field_callee(call)?;
        match root {
            "string" => classify_string_function(field),
            "os" if field == "date" => Some((DslKind::OsDate, ArgCount::None, 0)),
            _ => None,
        }
    }

    /// Resolve a dotted callee `root.field(...)` where `root` is a
    /// genuine global (not a shadowing local). Returns the two names.
    fn global_field_callee<'call>(
        &self,
        call: &'call FunctionCall,
    ) -> Option<(&'call str, &'call str)> {
        let Expression::Var(var) = &call.callee else {
            return None;
        };
        let Var::FieldAccess(field) = var else {
            return None;
        };
        let Expression::Var(prefix_var) = &field.prefix else {
            return None;
        };
        let Var::Name(prefix_token) = prefix_var else {
            return None;
        };
        let TokenKind::Identifier(prefix_name) = &prefix_token.kind else {
            return None;
        };
        // A shadowed root (`local string = ...`) is a user value.
        if self
            .semantic
            .resolves_to_local(prefix_name.as_str(), prefix_token.span)
        {
            return None;
        }
        let TokenKind::Identifier(field_name) = &field.name.kind else {
            return None;
        };
        Some((prefix_name.as_str(), field_name.as_str()))
    }

    /// Handle the method form `receiver:fn(args)` where the receiver is
    /// a string literal. The receiver IS the pattern in this case.
    fn check_method_literal_receiver(&mut self, call: &FunctionCall) {
        let Some(method_token) = &call.method else {
            return;
        };
        let TokenKind::Identifier(method_name) = &method_token.kind else {
            return;
        };
        let Some((dsl, arg_kind, _)) = classify_string_function(method_name.as_str()) else {
            return;
        };

        let receiver = unwrap_parens(&call.callee);
        let Expression::StringLiteral(receiver_token) = receiver else {
            return;
        };
        let lit_span = receiver_token.span;
        let Some((body, body_offset)) = pattern_body(self.source, lit_span) else {
            return;
        };

        let args_ref = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => Some(args),
            FunctionArgs::StringLiteral(_) | FunctionArgs::TableConstructor(_) => None,
        };

        // Note: for the method form, the literal is the RECEIVER and is
        // NOT counted as a positional arg - so substitution-count
        // expectations apply to the raw `args` punctuation as-is.
        match dsl {
            DslKind::Format => match validate_format(body) {
                Ok(specifier_count) => {
                    self.check_arg_count(arg_kind, args_ref, lit_span, specifier_count);
                }
                Err(err) => self.report_pattern_error(err, body_offset, "format"),
            },
            DslKind::LuaPattern => {
                if let Err(err) = validate_lua_pattern(body) {
                    self.report_pattern_error(err, body_offset, "pattern");
                }
            }
            DslKind::Pack => match validate_pack_format(body) {
                Ok(value_count) => {
                    self.check_arg_count(arg_kind, args_ref, lit_span, value_count);
                }
                Err(err) => self.report_pattern_error(err, body_offset, "pack format"),
            },
            // `classify_string_function` never yields `OsDate`; `os.date`
            // has no method form.
            DslKind::OsDate => {}
        }
    }

    /// Optional arg-count check. Skips silently if the call uses
    /// varargs anywhere in its tail.
    fn check_arg_count(
        &mut self,
        arg_kind: ArgCount,
        args: Option<&Punctuated<Expression>>,
        literal_span: Span,
        expected_extra: usize,
    ) {
        match arg_kind {
            ArgCount::ExactFormat | ArgCount::ExactPack => {}
            ArgCount::None => return,
        }
        let Some(args) = args else {
            // The method form passed `args_ref = None` to indicate the
            // receiver was the pattern. We still want to count the
            // method args directly; punt that path back to the caller.
            return;
        };
        // The literal itself sits at args index 0 in the dotted form;
        // skip it for the count. In the method form the receiver is the
        // pattern, so EVERY positional arg counts.
        let arg_list: Vec<&Expression> = args.iter().collect();
        let literal_idx = arg_list.iter().position(|expr| expr.span() == literal_span);

        // Bail if any trailing arg is a vararg (`...`) or a function
        // call - these expand to an unknown count at runtime.
        let trailing_iter = match literal_idx {
            Some(idx) => &arg_list[idx + 1..],
            None => &arg_list[..],
        };
        if trailing_iter
            .iter()
            .any(|expr| matches!(expr, Expression::VarArg(_) | Expression::FunctionCall(_)))
        {
            return;
        }

        let given = trailing_iter.len();
        if given == expected_extra {
            return;
        }
        self.out.push(LintDiagnostic::new(
            "format_string",
            format!("format expects {expected_extra} substitutions, given {given}"),
            literal_span,
        ));
    }

    /// Validate a literal `gsub` replacement string (third arg in the
    /// dotted form, second arg in the method form). The pattern arg
    /// contributes a capture count only when it is itself a literal.
    fn check_gsub_replacement(&mut self, call: &FunctionCall) {
        let (pattern_idx, repl_idx) = if let Some(method_token) = &call.method {
            let TokenKind::Identifier(method_name) = &method_token.kind else {
                return;
            };
            if method_name.as_str() != "gsub" {
                return;
            }
            (0, 1)
        } else {
            match self.global_field_callee(call) {
                Some(("string", "gsub")) => (1, 2),
                _ => return,
            }
        };
        let FunctionArgs::Parenthesized { args, .. } = &call.args else {
            return;
        };
        let Some(Expression::StringLiteral(repl_token)) = nth_expr(args, repl_idx) else {
            return;
        };
        let Some((repl_body, repl_offset)) = pattern_body(self.source, repl_token.span) else {
            return;
        };
        let capture_count = nth_expr(args, pattern_idx).and_then(|expr| match expr {
            Expression::StringLiteral(pattern_token) => {
                pattern_body(self.source, pattern_token.span)
                    .map(|(pattern_arg_body, _)| count_pattern_captures(pattern_arg_body))
            }
            _ => None,
        });
        if let Err((message, offset)) = validate_gsub_replacement(repl_body, capture_count) {
            self.report_literal_error(message, repl_offset + offset as u32, "replacement");
        }
    }

    fn report_literal_error(&mut self, message: &str, span_start: u32, kind: &str) {
        let span = Span::new(span_start, span_start + 1);
        self.out.push(LintDiagnostic::new(
            "format_string",
            format!("invalid {kind}: {message}"),
            span,
        ));
    }

    fn report_pattern_error(&mut self, err: PatternError, body_offset: u32, kind: &str) {
        let offset = pattern_error_offset(&err);
        let span_start = body_offset + offset;
        let span = Span::new(span_start, span_start + 1);
        self.out.push(LintDiagnostic::new(
            "format_string",
            format!("invalid {kind}: {err}"),
            span,
        ));
    }
}

impl NodeRule for FormatString {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset =
            AstTypesBitset::from_types(&[NodeType::FunctionCallStmt, NodeType::FunctionCallExpr]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::Statement,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::Statement::FunctionCall(call_stmt) = stmt {
            let mut checker = FormatChecker {
                source: ctx.source,
                semantic: ctx.semantic,
                out,
            };
            checker.check_call(&call_stmt.call);
            checker.check_method_literal_receiver(&call_stmt.call);
            checker.check_gsub_replacement(&call_stmt.call);
        }
    }
    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::FunctionCall(call) = expr {
            let mut checker = FormatChecker {
                source: ctx.source,
                semantic: ctx.semantic,
                out,
            };
            checker.check_call(call);
            checker.check_method_literal_receiver(call);
            checker.check_gsub_replacement(call);
        }
    }
}

fn classify_string_function(name: &str) -> Option<(DslKind, ArgCount, usize)> {
    match name {
        "format" => Some((DslKind::Format, ArgCount::ExactFormat, 0)),
        // Pattern-based: arg-count varies (extra `init`, replacement,
        // etc.) so we don't enforce.
        "match" | "gmatch" | "find" | "gsub" => Some((DslKind::LuaPattern, ArgCount::None, 1)),
        "pack" | "packsize" | "unpack" => Some((DslKind::Pack, ArgCount::ExactPack, 0)),
        _ => None,
    }
}

/// Walk past any parens wrapping an expression - `("x"):format(...)`.
fn unwrap_parens(expr: &Expression) -> &Expression {
    let mut current = expr;
    while let Expression::Parenthesized(node) = current {
        current = &node.expr;
    }
    current
}

/// Extract the raw body of a string literal token. Returns the body
/// slice and the byte offset (into source) where that body starts.
/// Returns `None` for long-bracket strings - escape rules make the
/// raw-body mapping unreliable for those.
fn pattern_body(source: &str, span: Span) -> Option<(&str, u32)> {
    let start = span.start as usize;
    let end = span.end as usize;
    if start >= end || end > source.len() {
        return None;
    }
    let slice = &source[start..end];
    let bytes = slice.as_bytes();
    let first = *bytes.first()?;
    if first == b'"' || first == b'\'' {
        if bytes.len() < 2 || *bytes.last()? != first {
            return None;
        }
        let body = &slice[1..slice.len() - 1];
        Some((body, span.start + 1))
    } else {
        None
    }
}

/// Count capture groups in a Lua pattern: unescaped `(`, including
/// position captures `()`. Character sets are skipped so `[(]` does not
/// count. `%b()` parens are counted even though they are not captures -
/// overcounting only suppresses diagnostics, never invents them.
fn count_pattern_captures(pattern: &str) -> usize {
    let bytes = pattern.as_bytes();
    let mut count = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => i += 2,
            b'[' => {
                i += 1;
                if bytes.get(i) == Some(&b'^') {
                    i += 1;
                }
                // A `]` right after the opener (or `^`) is a literal.
                if bytes.get(i) == Some(&b']') {
                    i += 1;
                }
                while i < bytes.len() && bytes[i] != b']' {
                    if bytes[i] == b'%' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'(' => {
                count += 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    count
}

/// Validate a `gsub` replacement string. `capture_count` is `None` when
/// the pattern arg is not a literal, which disables the index check.
/// `%0` is the whole match, and with zero captures `%1` also refers to
/// the whole match, so only indices above `max(captures, 1)` are flagged.
fn validate_gsub_replacement(
    body: &str,
    capture_count: Option<usize>,
) -> Result<(), (&'static str, usize)> {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            i += 1;
            continue;
        }
        let Some(&next) = bytes.get(i + 1) else {
            return Err(("unfinished replacement", i));
        };
        if next == b'%' {
            i += 2;
            continue;
        }
        if !next.is_ascii_digit() {
            return Err((
                "unexpected replacement character; must be a digit or %",
                i + 1,
            ));
        }
        if let Some(captures) = capture_count {
            let index = (next - b'0') as usize;
            if index > captures.max(1) {
                return Err((
                    "invalid capture index, must refer to a pattern capture",
                    i + 1,
                ));
            }
        }
        i += 2;
    }
    Ok(())
}

/// Validate an `os.date` format string: optional leading `!` (UTC),
/// then either exactly `*t` (table request) or text with `%`-specifiers
/// drawn from the C89 strftime set.
fn validate_os_date(body: &str) -> Result<(), (&'static str, usize)> {
    if let Some(pos) = body.find('\0') {
        return Err(("can not contain null characters", pos));
    }
    let bytes = body.as_bytes();
    let mut i = 0;
    if bytes.first() == Some(&b'!') {
        i = 1;
    }
    if &body[i..] == "*t" {
        return Ok(());
    }
    while i < bytes.len() {
        if bytes[i] != b'%' {
            i += 1;
            continue;
        }
        let Some(&next) = bytes.get(i + 1) else {
            return Err(("unfinished replacement", i));
        };
        if next != b'%' && !b"aAbBcdHIjmMpSUwWxXyYzZ".contains(&next) {
            return Err((
                "unexpected replacement character; must be a date format specifier or %",
                i + 1,
            ));
        }
        i += 2;
    }
    Ok(())
}

/// Walk a punctuated list and pick the nth expression (0-indexed).
fn nth_expr(args: &Punctuated<Expression>, idx: usize) -> Option<&Expression> {
    args.iter().nth(idx)
}

/// Pull the byte offset out of any `PatternError` variant.
fn pattern_error_offset(err: &PatternError) -> u32 {
    let offset = match err {
        PatternError::TruncatedSpecifier { offset }
        | PatternError::UnterminatedSet { offset }
        | PatternError::EmptySet { offset }
        | PatternError::InvalidQuantifierTarget { offset }
        | PatternError::UnmatchedCapture { offset }
        | PatternError::TruncatedPackSize { offset }
        | PatternError::BadWidth { offset }
        | PatternError::BadPrecision { offset } => *offset,
        PatternError::UnknownSpecifier { offset, .. }
        | PatternError::BadFlag { offset, .. }
        | PatternError::BadEscape { offset, .. }
        | PatternError::InvalidPackOption { offset, .. } => *offset,
    };
    offset as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&FormatString, source, LuaVersion::Lua54)
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
        assert!(
            diags[0].message.contains("expects 1 substitutions"),
            "msg: {}",
            diags[0].message
        );
    }

    #[test]
    fn flags_unknown_specifier() {
        let diags = run("string.format(\"%z\", 1)");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(
            diags[0].message.contains("unknown specifier"),
            "msg: {}",
            diags[0].message
        );
    }

    #[test]
    fn flags_pattern_unterminated_set() {
        let diags = run("string.match(\"abc\", \"[abc\")");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(
            diags[0].message.contains("unterminated set"),
            "msg: {}",
            diags[0].message
        );
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
        assert!(
            diags[0].message.contains("invalid pack option"),
            "msg: {}",
            diags[0].message
        );
    }

    #[test]
    fn ignores_method_form_correct_format() {
        // The receiver `"hello"` IS the format string; arg count should
        // match the spec count (one `%d`, one trailing arg).
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
        // We can't validate a variable's content; no diagnostic.
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
        // `f()` expands to an unknown count at runtime, so we can't tell.
        let diags = run("local function f() end\nstring.format(\"%d %d\", f())");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_long_bracket_strings() {
        // Long brackets aren't validated (escape mapping is unreliable).
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
        // %% escapes, %0 is the whole match, %1 refers to the first
        // capture (or the whole match when the pattern has none).
        let diags = run("string.gsub(\"x\", \"(a)\", \"%%%0%1\")");
        assert!(diags.is_empty(), "got: {diags:?}");
        let diags = run("string.gsub(\"x\", \"a\", \"%1\")");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_gsub_nonliteral_pattern_index() {
        // Unknown capture count: index checking is disabled.
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
}
