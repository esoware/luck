use luck_ast::Expression;
use luck_ast::expr::{FunctionArgs, FunctionCall};
use luck_ast::visitor::Visitor;
use luck_semantic::SemanticAnalysis;
use luck_semantic::stdlib_model::{
    EntryKind, StdlibDeprecation, StdlibEntry, expand_replace_template,
};
use luck_token::Span;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

pub struct Deprecated;

impl Rule for Deprecated {
    fn name(&self) -> &'static str {
        "deprecated"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "use of deprecated standard library function"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let semantic = ctx.semantic;
        let source = ctx.source;
        let _comments = ctx.comments;
        let mut checker = DeprecatedChecker {
            source,
            semantic,
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct DeprecatedChecker<'a> {
    source: &'a str,
    semantic: &'a SemanticAnalysis,
    diagnostics: Vec<LintDiagnostic>,
}

impl<'src> DeprecatedChecker<'src> {
    fn check_call(&mut self, call: &FunctionCall, is_statement: bool) {
        // Method calls (`obj:method(...)`) hit instance metatables, not
        // stdlib paths - we can't resolve them with confidence.
        if call.method.is_some() {
            return;
        }

        let Some((segments, display_name)) = self.resolve_callee_path(&call.callee) else {
            return;
        };

        let Some(entry) = self.semantic.lookup_stdlib_str(&segments) else {
            return;
        };

        let Some(deprecation) = function_deprecation(entry) else {
            return;
        };

        let fix = self.build_fix(call, deprecation, &display_name, is_statement);
        self.diagnostics.push(
            LintDiagnostic::new(
                "deprecated",
                format!("'{display_name}' is deprecated"),
                call.span,
            )
            .with_help(deprecation.message.to_string())
            .with_fix_opt(fix),
        );
    }

    fn resolve_callee_path(&self, expr: &Expression) -> Option<(Vec<&'src str>, String)> {
        let Expression::Var(var) = expr else {
            return None;
        };
        match var.as_ref() {
            luck_ast::expr::Var::Name(token) => {
                let name = self.slice(token.span);
                // Shadowed base names are user values, not the stdlib.
                if self.semantic.resolves_to_local(name, token.span) {
                    return None;
                }
                Some((vec![name], name.to_string()))
            }
            luck_ast::expr::Var::FieldAccess(field_access) => {
                let Expression::Var(prefix_var) = &field_access.prefix else {
                    return None;
                };
                let luck_ast::expr::Var::Name(prefix_token) = prefix_var.as_ref() else {
                    return None;
                };
                let prefix = self.slice(prefix_token.span);
                // Shadowed base names are user values, not the stdlib.
                if self.semantic.resolves_to_local(prefix, prefix_token.span) {
                    return None;
                }
                let field = self.slice(field_access.name.span);
                Some((vec![prefix, field], format!("{prefix}.{field}")))
            }
            _ => None,
        }
    }

    fn build_fix(
        &self,
        call: &FunctionCall,
        deprecation: &StdlibDeprecation,
        display_name: &str,
        is_statement: bool,
    ) -> Option<Fix> {
        let template = deprecation.replace_template.as_ref()?;
        let args = self.collect_arg_slices(&call.args)?;
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let replacement = expand_replace_template(template, &arg_refs);
        // Templates like `(%1 ^ %2)` expand to a bare expression, which
        // cannot replace a call standing alone as a statement (fixed
        // output must re-parse). Keep the diagnostic, drop the fix.
        if is_statement
            && !luck_parser::parse(&replacement, self.semantic.version)
                .errors
                .is_empty()
        {
            return None;
        }
        Some(Fix {
            description: format!("replace deprecated '{display_name}' with '{replacement}'"),
            edits: vec![TextEdit {
                span: call.span,
                replacement,
            }],
        })
    }

    fn collect_arg_slices(&self, args: &FunctionArgs) -> Option<Vec<String>> {
        match args {
            FunctionArgs::Parenthesized {
                args: punctuated, ..
            } => Some(
                punctuated
                    .iter()
                    .map(|expr| self.slice(expr.span()).to_string())
                    .collect(),
            ),
            FunctionArgs::StringLiteral(token) => Some(vec![self.slice(token.span).to_string()]),
            FunctionArgs::TableConstructor(table) => Some(vec![self.slice(table.span).to_string()]),
        }
    }

    fn slice(&self, span: Span) -> &'src str {
        &self.source[span.start as usize..span.end as usize]
    }
}

fn function_deprecation(entry: &StdlibEntry) -> Option<&StdlibDeprecation> {
    match &entry.kind {
        EntryKind::Function(func) => func.deprecated.as_ref(),
        EntryKind::Constant(value) | EntryKind::Property(value) => value.deprecated.as_ref(),
        EntryKind::Namespace(_) => None,
    }
}

impl Visitor for DeprecatedChecker<'_> {
    fn visit_statement(&mut self, stmt: &luck_ast::Statement) {
        if let luck_ast::Statement::FunctionCall(call_stmt) = stmt {
            self.check_call(&call_stmt.call, true);
        }
        self.walk_statement(stmt);
    }

    fn visit_expression(&mut self, expr: &Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.check_call(call, false);
        }
        self.walk_expression(expr);
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::diagnostic::Fix;

    use super::Deprecated;

    /// Run the rule on a snippet at a given Lua version and return the
    /// auto-fix description + applied source (the `%n`-expanded
    /// replacement substituted over the original call span). Returns
    /// `None` if no fix is produced.
    fn run_fix(source: &str, version: LuaVersion) -> Option<(String, String)> {
        let diags = crate::test_support::run_rule(&Deprecated, source, version);
        let diag = diags.into_iter().find(|d| d.fix.is_some())?;
        let Fix { description, edits } = diag.fix.unwrap();
        // Every fix produces exactly one edit, so applying the first one in place is sufficient.
        let edit = edits.into_iter().next()?;
        let mut applied = String::with_capacity(source.len());
        applied.push_str(&source[..edit.span.start as usize]);
        applied.push_str(&edit.replacement);
        applied.push_str(&source[edit.span.end as usize..]);
        let parse = luck_parser::parse(&applied, version);
        assert!(parse.errors.is_empty(), "reparse: {:?}", parse.errors);
        Some((description, applied))
    }

    #[test]
    fn snapshot_loadstring_to_load() {
        let (desc, after) = run_fix("loadstring('x')", LuaVersion::Lua54).unwrap();
        assert_eq!(desc, "replace deprecated 'loadstring' with 'load('x')'");
        assert_eq!(after, "load('x')");
    }

    #[test]
    fn snapshot_string_gfind_to_gmatch() {
        let (desc, after) = run_fix("string.gfind(s, p)", LuaVersion::Lua54).unwrap();
        assert_eq!(
            desc,
            "replace deprecated 'string.gfind' with 'string.gmatch(s, p)'"
        );
        assert_eq!(after, "string.gmatch(s, p)");
    }

    #[test]
    fn snapshot_table_getn_to_length() {
        let (desc, after) = run_fix("table.getn(t)", LuaVersion::Lua54).unwrap();
        assert_eq!(desc, "replace deprecated 'table.getn' with '#t'");
        assert_eq!(after, "#t");
    }

    #[test]
    fn snapshot_unpack_to_table_unpack() {
        let (desc, after) = run_fix("unpack(t)", LuaVersion::Lua54).unwrap();
        assert_eq!(desc, "replace deprecated 'unpack' with 'table.unpack(t)'");
        assert_eq!(after, "table.unpack(t)");
    }

    #[test]
    fn snapshot_math_pow_to_caret() {
        let (desc, after) = run_fix("local r = math.pow(a, b)", LuaVersion::Lua54).unwrap();
        assert_eq!(desc, "replace deprecated 'math.pow' with '(a ^ b)'");
        assert_eq!(after, "local r = (a ^ b)");
    }

    #[test]
    fn no_fix_for_expression_template_in_statement_position() {
        // A bare `math.pow(a, b)` statement cannot be replaced by the
        // expression `(a ^ b)`; the diagnostic must fire without a fix.
        let diags = crate::test_support::run_rule(&Deprecated, "math.pow(a, b)", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "expected no fix: {diags:?}");
        assert!(run_fix("math.pow(a, b)", LuaVersion::Lua54).is_none());
    }

    #[test]
    fn snapshot_math_log10_to_log_base_10() {
        let (desc, after) = run_fix("math.log10(x)", LuaVersion::Lua54).unwrap();
        assert_eq!(
            desc,
            "replace deprecated 'math.log10' with 'math.log(x, 10)'"
        );
        assert_eq!(after, "math.log(x, 10)");
    }

    #[test]
    fn ignores_method_call() {
        // `obj:method(...)` shouldn't resolve to a stdlib path even if
        // the method name matches a deprecated entry - we can't know
        // what type `obj` is, so no diagnostic.
        let diags = crate::test_support::run_rule(&Deprecated, "obj:getn()", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
