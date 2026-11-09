use luck_ast::{Expression, Statement, Var};
use luck_token::{LuaVersion, Span, TokenKind, short_string_body};

use crate::diagnostic::Finding;
use crate::fix::{Fix, TextEdit};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct StringIndexToField;

impl_rule!(
    StringIndexToField,
    "string_index_to_field",
    Style,
    Warning,
    "use of string-key index for an identifier-safe key; prefer dot syntax"
);

impl NodeRule for StringIndexToField {
    // Assignment targets are `Var`s but not expressions, so the statements
    // that hold them are checked too.
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[
            NodeType::Var,
            NodeType::Assignment,
            NodeType::CompoundAssignment,
        ])
    }
    fn on_stmt(&self, stmt: &Statement, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let version = context.semantic.version;
        match stmt {
            Statement::Assignment(assignment) => {
                for target in assignment.targets.iter() {
                    findings.extend(check_var(target, version));
                }
            }
            // Luau
            Statement::CompoundAssignment(compound_assignment) => {
                findings.extend(check_var(&compound_assignment.var, version));
            }
            Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }
    fn on_expr(&self, expr: &Expression, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        if let Expression::Var(var) = expr {
            findings.extend(check_var(var, context.semantic.version));
        }
    }
}

fn check_var(var: &Var, version: LuaVersion) -> Option<Finding> {
    let Var::Index(index_expr) = var else {
        return None;
    };
    let Expression::StringLiteral(literal) = &index_expr.index else {
        return None;
    };
    // The body is the raw spelling. The shape check rejects the `\` of any
    // escape, which could make the key differ from that spelling.
    let name = short_string_body(&literal.text)?;
    if !TokenKind::is_identifier_shaped(name) || TokenKind::is_reserved(name, version) {
        return None;
    }

    // Replace everything after the prefix, brackets included.
    let span = Span::new(index_expr.prefix.span().end, index_expr.span.end);
    Some(
        Finding::new(format!("`[\"{name}\"]` is equivalent to `.{name}`"), span)
            .with_help("prefer dot access for identifier-safe keys")
            .with_fix(Fix {
                description: format!("rewrite `[\"{name}\"]` as `.{name}`"),
                edits: vec![TextEdit { span, replacement: format!(".{name}") }],
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&StringIndexToField, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_identifier_safe_key() {
        let source = "local x = t[\"foo\"]";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local x = t.foo");
    }

    #[test]
    fn ignores_keyword_key() {
        let diags = run("local x = t[\"end\"]");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_luau_continue_key() {
        let diags = run_rule(&StringIndexToField, "local x = t[\"continue\"]", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
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
    fn flags_each_nested_index() {
        let source = "local x = t[\"a\"][\"b\"]";
        let diags = run(source);
        assert_eq!(diags.len(), 2, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local x = t.a.b");
    }

    #[test]
    fn flags_assignment_targets() {
        let source = "t[\"a\"][\"b\"], u[\"c\"] = 1, 2";
        let diags = run(source);
        assert_eq!(diags.len(), 3, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "t.a.b, u.c = 1, 2");
    }

    #[test]
    fn flags_compound_assignment_target() {
        let source = "t[\"count\"] += 1";
        let diags = run_rule(&StringIndexToField, source, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(fixed, "t.count += 1");
    }

    #[test]
    fn ignores_escape_sequence_key() {
        let diags = run("local x = t[\"a\\nb\"]");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_underscore_prefixed_key() {
        // A leading `_` is valid in an identifier.
        let source = "local x = t[\"_priv\"]";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local x = t._priv");
    }
}
