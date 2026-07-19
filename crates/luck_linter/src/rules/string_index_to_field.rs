use luck_ast::Expression;
use luck_ast::expr::{IndexExpression, Var};
use luck_ast::visitor::Visitor;
use luck_token::{LuaVersion, Span, TokenKind};

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// `t["foo"]` where `foo` is a valid Lua identifier (not a keyword) is
/// the same as `t.foo` - the dot form reads cleaner.
pub struct StringIndexToField;

impl Rule for StringIndexToField {
    fn name(&self) -> &'static str {
        "string_index_to_field"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "use of string-key index for an identifier-safe key; prefer dot syntax"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let semantic = ctx.semantic;
        let source = ctx.source;
        let _comments = ctx.comments;
        let mut checker = IndexChecker {
            source,
            version: semantic.version,
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct IndexChecker<'src> {
    source: &'src str,
    version: LuaVersion,
    diagnostics: Vec<LintDiagnostic>,
}

impl IndexChecker<'_> {
    fn check_index(&mut self, idx: &IndexExpression) {
        let Expression::StringLiteral(token) = &idx.index else {
            return;
        };
        let TokenKind::StringLiteral(_) = &token.kind else {
            return;
        };

        // We need the raw source slice - the CompactString in the token
        // is the lexed value, but the source preserves the quotes and
        // escapes. The literal must be a simple short-string with no
        // escape sequences and must contain only identifier chars.
        let raw = &self.source[token.span.start as usize..token.span.end as usize];
        let Some(name) = identifier_inside(raw) else {
            return;
        };
        if !is_ident(name) {
            return;
        }
        if is_reserved(name, self.version) {
            return;
        }

        // The fix replaces the bracket-and-string span `[ "name" ]`
        // with `.name`. Anchor the replacement at the opening bracket;
        // the index expression's span already covers the whole bracket
        // pair plus its contents.
        let open_byte = idx.brackets.open.span.start;
        let close_byte = idx.brackets.close.span.end;
        let replacement = format!(".{name}");

        self.diagnostics.push(
            LintDiagnostic::new(
                "string_index_to_field",
                format!("`[\"{name}\"]` is equivalent to `.{name}`"),
                Span::new(open_byte, close_byte),
            )
            .with_help("prefer dot access for identifier-safe keys".to_string())
            .with_fix(Fix {
                description: format!("rewrite `[\"{name}\"]` as `.{name}`"),
                edits: vec![TextEdit {
                    span: Span::new(open_byte, close_byte),
                    replacement,
                }],
            }),
        );
    }
}

/// Strip matching short-string delimiters and return the inner text iff
/// the literal has no escape sequences (those could imply the key isn't
/// actually `name`).
fn identifier_inside(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let first = bytes[0];
    let last = bytes[bytes.len() - 1];
    if first != last || (first != b'"' && first != b'\'') {
        return None;
    }
    let inner = &raw[1..raw.len() - 1];
    if inner.contains('\\') {
        return None;
    }
    Some(inner)
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Lua keywords are version-specific in a couple of cases (`goto`/`global`).
fn is_reserved(name: &str, version: LuaVersion) -> bool {
    matches!(
        name,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
    ) || (name == "goto" && version.has_goto())
        || (name == "global" && version.has_global())
        || (name == "continue" && version.has_continue())
}

impl Visitor for IndexChecker<'_> {
    fn visit_var(&mut self, var: &Var) {
        if let Var::Index(idx) = var {
            self.check_index(idx);
        }
        self.walk_var(var);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&StringIndexToField, source, LuaVersion::Lua54)
    }

    fn apply_all(source: &str, diags: &[LintDiagnostic]) -> String {
        let mut edits: Vec<&TextEdit> = diags
            .iter()
            .filter_map(|d| d.fix.as_ref())
            .flat_map(|f| &f.edits)
            .collect();
        edits.sort_by_key(|e| std::cmp::Reverse(e.span.start));
        let mut out = source.to_string();
        for edit in edits {
            out.replace_range(
                edit.span.start as usize..edit.span.end as usize,
                &edit.replacement,
            );
        }
        let parse = luck_parser::parse(&out, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "reparse: {:?}", parse.errors);
        out
    }

    #[test]
    fn flags_identifier_safe_key() {
        let source = "local x = t[\"foo\"]";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply_all(source, &diags);
        assert_eq!(fixed, "local x = t.foo");
    }

    #[test]
    fn ignores_keyword_key() {
        let diags = run("local x = t[\"end\"]");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_non_identifier_key() {
        let diags = run("local x = t[\"with space\"]");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_numeric_start() {
        let diags = run("local x = t[\"1bad\"]");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn handles_nested_index() {
        let source = "local x = t[\"a\"][\"b\"]";
        let diags = run(source);
        assert_eq!(diags.len(), 2, "{diags:?}");
        let fixed = apply_all(source, &diags);
        assert_eq!(fixed, "local x = t.a.b");
    }

    #[test]
    fn ignores_escape_sequence_key() {
        let diags = run("local x = t[\"a\\nb\"]");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_underscore_prefixed_key() {
        // `_` is a valid identifier; the rule should rewrite.
        let source = "local x = t[\"_priv\"]";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply_all(source, &diags);
        assert_eq!(fixed, "local x = t._priv");
    }
}
