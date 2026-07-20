use luck_ast::Expression;
use luck_ast::expr::{FunctionArgs, FunctionCall, Var};
use luck_ast::node::{AstTypesBitset, NodeType};
use luck_semantic::SemanticAnalysis;
use luck_semantic::stdlib_model::{
    StdlibArgKind, StdlibDeprecation, StdlibEntry, StdlibFunction, expand_replace_template,
};
use luck_token::Span;

use super::incorrect_stdlib_use::string_literal_value;
use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};

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
        crate::bus::run_single(self, ctx)
    }
}

struct DeprecatedChecker<'src, 'out> {
    source: &'src str,
    semantic: &'src SemanticAnalysis,
    out: &'out mut Vec<LintDiagnostic>,
}

impl<'src> DeprecatedChecker<'src, '_> {
    fn check_call(&mut self, call: &FunctionCall, is_statement: bool) {
        let Some((display_name, resolved)) = self.semantic.resolve_callee(call) else {
            return;
        };

        if let Some(deprecation) = resolved.entry.deprecation() {
            let fix = self.build_fix(call, deprecation, &display_name, is_statement);
            self.out.push(
                LintDiagnostic::new(
                    "deprecated",
                    format!("`{display_name}` is deprecated"),
                    call.span,
                )
                .with_help(deprecation.message.to_string())
                .with_fix_opt(fix),
            );
            return;
        }

        if let StdlibEntry::Function(func) = resolved.entry {
            self.check_deprecated_constants(call, &display_name, func);
            self.check_deprecated_params(call, &display_name, func);
        }
    }

    /// A live function called with an argument in a position every
    /// arity-matching signature marks deprecated (e.g. the `parent`
    /// arg of `Instance.new`). Fires on any expression, not just
    /// literals - passing the position at all is the problem.
    fn check_deprecated_params(
        &mut self,
        call: &FunctionCall,
        display_name: &str,
        func: &StdlibFunction,
    ) {
        let FunctionArgs::Parenthesized { args, .. } = &call.args else {
            return;
        };
        let arg_count = args.len();
        for (idx, expr) in args.iter().enumerate() {
            let mut deprecation: Option<&StdlibDeprecation> = None;
            let mut live = false;
            for sig in func.matching_signatures(arg_count) {
                match sig
                    .params
                    .get(idx)
                    .and_then(|param| param.deprecated.as_ref())
                {
                    Some(dep) => deprecation = deprecation.or(Some(dep)),
                    None => live = true,
                }
            }
            if live {
                continue;
            }
            if let Some(deprecation) = deprecation {
                self.out.push(
                    LintDiagnostic::new(
                        "deprecated",
                        format!("argument {} of `{display_name}` is deprecated", idx + 1),
                        expr.span(),
                    )
                    .with_help(deprecation.message.to_string()),
                );
            }
        }
    }

    /// A live function called with a deprecated constant value (e.g. a
    /// dead service name in `game:GetService(...)`, or
    /// `collectgarbage("setpause")` in 5.4). Same matching discipline
    /// as the constant-set arity check: only literal strings, only
    /// positions constrained in every signature accepting this arity.
    fn check_deprecated_constants(
        &mut self,
        call: &FunctionCall,
        display_name: &str,
        func: &StdlibFunction,
    ) {
        let positional_args: Vec<&Expression> = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => args.iter().collect(),
            FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => Vec::new(),
        };
        let arg_count = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => args.len(),
            FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => 1,
        };
        for (idx, expr) in positional_args.iter().enumerate() {
            let Some((value, span)) = string_literal_value(expr, self.source) else {
                continue;
            };
            let mut deprecation: Option<&StdlibDeprecation> = None;
            let mut constrained = false;
            let mut unconstrained = false;
            for sig in func.matching_signatures(arg_count) {
                match sig.params.get(idx).map(|param| &param.kind) {
                    Some(StdlibArgKind::Constant(values)) => {
                        constrained = true;
                        if let Some(constant) =
                            values.iter().find(|constant| constant.value == value)
                        {
                            deprecation = deprecation.or(constant.deprecated.as_ref());
                        }
                    }
                    _ => unconstrained = true,
                }
            }
            if !constrained || unconstrained {
                continue;
            }
            if let Some(deprecation) = deprecation {
                self.out.push(
                    LintDiagnostic::new(
                        "deprecated",
                        format!("`{value}` is a deprecated argument of `{display_name}`"),
                        span,
                    )
                    .with_help(deprecation.message.to_string()),
                );
            }
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
            description: format!("replace deprecated `{display_name}` with `{replacement}`"),
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

impl DeprecatedChecker<'_, '_> {
    /// Deprecated value READS: a dotted access resolving to a
    /// deprecated constant, property, or namespace (deprecated Roblox
    /// enum items and types). Functions are excluded here - their use
    /// site is the call, which `check_call` already reports.
    fn check_field_access(&mut self, expr: &Expression) {
        let Expression::Var(Var::FieldAccess(_)) = expr else {
            return;
        };
        let Some((segments, spans)) = crate::path::dotted_path(expr) else {
            return;
        };
        if self.semantic.resolves_to_local(segments[0], spans[0]) {
            return;
        }
        let Some(entry) = self.semantic.stdlib().lookup_str(&segments) else {
            return;
        };
        if matches!(entry, StdlibEntry::Function(_)) {
            return;
        }
        // Only this node's own entry: a deprecated prefix is reported
        // by the inner access node, so chains warn exactly once.
        let Some(deprecation) = entry.deprecation() else {
            return;
        };
        let display_name = segments.join(".");
        self.out.push(
            LintDiagnostic::new(
                "deprecated",
                format!("`{display_name}` is deprecated"),
                *spans.last().expect("span per segment"),
            )
            .with_help(deprecation.message.to_string()),
        );
    }
}

impl NodeRule for Deprecated {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[
            NodeType::FunctionCallStmt,
            NodeType::FunctionCallExpr,
            NodeType::Var,
        ]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::Statement,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::Statement::FunctionCall(call_stmt) = stmt {
            DeprecatedChecker {
                source: ctx.source,
                semantic: ctx.semantic,
                out,
            }
            .check_call(&call_stmt.call, true);
        }
    }
    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        let mut checker = DeprecatedChecker {
            source: ctx.source,
            semantic: ctx.semantic,
            out,
        };
        match expr {
            Expression::FunctionCall(call) => checker.check_call(call, false),
            Expression::Var(_) => checker.check_field_access(expr),
            _ => {}
        }
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
        assert_eq!(desc, "replace deprecated `loadstring` with `load('x')`");
        assert_eq!(after, "load('x')");
    }

    #[test]
    fn snapshot_string_gfind_to_gmatch() {
        let (desc, after) = run_fix("string.gfind(s, p)", LuaVersion::Lua54).unwrap();
        assert_eq!(
            desc,
            "replace deprecated `string.gfind` with `string.gmatch(s, p)`"
        );
        assert_eq!(after, "string.gmatch(s, p)");
    }

    #[test]
    fn snapshot_table_getn_to_length() {
        let (desc, after) = run_fix("table.getn(t)", LuaVersion::Lua54).unwrap();
        assert_eq!(desc, "replace deprecated `table.getn` with `#t`");
        assert_eq!(after, "#t");
    }

    #[test]
    fn snapshot_unpack_to_table_unpack() {
        let (desc, after) = run_fix("unpack(t)", LuaVersion::Lua54).unwrap();
        assert_eq!(desc, "replace deprecated `unpack` with `table.unpack(t)`");
        assert_eq!(after, "table.unpack(t)");
    }

    #[test]
    fn snapshot_math_pow_to_caret() {
        let (desc, after) = run_fix("local r = math.pow(a, b)", LuaVersion::Lua54).unwrap();
        assert_eq!(desc, "replace deprecated `math.pow` with `(a ^ b)`");
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
            "replace deprecated `math.log10` with `math.log(x, 10)`"
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

    #[test]
    fn flags_deprecated_string_method_on_literal() {
        // The derived string receiver keeps entry deprecation, so the
        // colon form of gfind warns like the dotted one.
        let diags = crate::test_support::run_rule(
            &Deprecated,
            "local it = ('x'):gfind('a')",
            LuaVersion::Lua51,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("gfind")),
            "{diags:?}"
        );
    }

    #[test]
    fn flags_dead_service_name() {
        let diags = crate::test_support::run_rule_roblox(
            &Deprecated,
            "local p = game:GetService('PointsService')",
        );
        assert!(
            diags.iter().any(|d| d
                .message
                .contains("`PointsService` is a deprecated argument")),
            "{diags:?}"
        );
    }

    #[test]
    fn ignores_live_service_name() {
        let diags = crate::test_support::run_rule_roblox(
            &Deprecated,
            "local p = game:GetService('Players')",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_deprecated_instance_new_parent_arg() {
        let diags = crate::test_support::run_rule_roblox(
            &Deprecated,
            "local p = Instance.new('Part', workspace)",
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("argument 2 of `Instance.new`")),
            "{diags:?}"
        );
    }

    #[test]
    fn ignores_instance_new_without_parent_arg() {
        let diags =
            crate::test_support::run_rule_roblox(&Deprecated, "local p = Instance.new('Part')");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_deprecated_collectgarbage_option() {
        // Per-constant deprecation from the 5.4 GC rework.
        let diags = crate::test_support::run_rule(
            &Deprecated,
            "collectgarbage('setpause', 100)",
            LuaVersion::Lua54,
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("`setpause` is a deprecated argument")),
            "{diags:?}"
        );
    }
}
