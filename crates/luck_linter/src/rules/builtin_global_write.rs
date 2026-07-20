use luck_ast::Statement;
use luck_ast::expr::{Expression, Var};
use luck_ast::stmt::FuncName;
use luck_ast::visitor::Visitor;
use luck_semantic::SemanticAnalysis;
use luck_semantic::scope::ReferenceKind;
use luck_token::{Span, TokenKind};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, Rule};

pub struct BuiltinGlobalWrite;

impl Rule for BuiltinGlobalWrite {
    fn name(&self) -> &'static str {
        "builtin_global_write"
    }

    fn category(&self) -> Category {
        Category::Correctness
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "overwriting a standard-library global or one of its fields"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let semantic = ctx.semantic;
        let mut diagnostics = Vec::new();

        for reference in semantic.scope_tree.unresolved_references() {
            if !matches!(
                reference.kind,
                ReferenceKind::Write | ReferenceKind::ReadWrite
            ) {
                continue;
            }
            if !is_overwritable_builtin(semantic, &reference.name) {
                continue;
            }
            diagnostics.push(diagnostic(&reference.name, reference.span));
        }

        let mut checker = FieldWriteChecker {
            semantic,
            diagnostics,
        };
        checker.visit_block(ctx.block);
        checker.diagnostics
    }
}

/// Extra-globals from config are user-declared names, so overwriting them
/// is intentional even when they collide with a stdlib name. Globals the
/// model marks writable (`_G`, `shared` on Roblox, `_PROMPT`) are the
/// documented set-me idiom, not an overwrite.
fn is_overwritable_builtin(semantic: &SemanticAnalysis, name: &str) -> bool {
    semantic.is_known_global(name)
        && semantic.is_read_only_global(name)
        && !semantic.extra_globals.contains(name)
}

fn diagnostic(name: &str, span: Span) -> LintDiagnostic {
    LintDiagnostic::new(
        "builtin_global_write",
        format!(
            "built-in global `{name}` is overwritten here; consider a local or a different name"
        ),
        span,
    )
}

fn expr_base_ident(expr: &Expression) -> Option<(&str, Span)> {
    match expr {
        Expression::Var(var) => match var {
            Var::Name(token) => match &token.kind {
                TokenKind::Identifier(name) => Some((name.as_str(), token.span)),
                _ => None,
            },
            Var::FieldAccess(field_access) => expr_base_ident(&field_access.prefix),
            Var::Index(index) => expr_base_ident(&index.prefix),
        },
        _ => None,
    }
}

fn var_dotted_segments(var: &Var) -> Option<Vec<String>> {
    match var {
        Var::Name(token) => match &token.kind {
            TokenKind::Identifier(name) => Some(vec![name.to_string()]),
            _ => None,
        },
        Var::FieldAccess(field_access) => {
            let Expression::Var(inner) = &field_access.prefix else {
                return None;
            };
            let mut segments = var_dotted_segments(inner)?;
            match &field_access.name.kind {
                TokenKind::Identifier(name) => {
                    segments.push(name.to_string());
                    Some(segments)
                }
                _ => None,
            }
        }
        Var::Index(_) => None,
    }
}

fn var_dotted_path(var: &Var) -> Option<String> {
    match var {
        Var::Name(token) => match &token.kind {
            TokenKind::Identifier(name) => Some(name.to_string()),
            _ => None,
        },
        Var::FieldAccess(field_access) => {
            let Expression::Var(inner) = &field_access.prefix else {
                return None;
            };
            let prefix = var_dotted_path(inner)?;
            match &field_access.name.kind {
                TokenKind::Identifier(name) => Some(format!("{prefix}.{name}")),
                _ => None,
            }
        }
        Var::Index(_) => None,
    }
}

struct FieldWriteChecker<'a> {
    semantic: &'a SemanticAnalysis,
    diagnostics: Vec<LintDiagnostic>,
}

impl FieldWriteChecker<'_> {
    fn check_var_target(&mut self, var: &Var) {
        let (base, span) = match var {
            // Whole-name writes surface as unresolved Write references.
            Var::Name(_) => return,
            Var::Index(index) => (expr_base_ident(&index.prefix), index.span),
            Var::FieldAccess(field_access) => {
                (expr_base_ident(&field_access.prefix), field_access.span)
            }
        };
        let Some((name, name_span)) = base else {
            return;
        };
        if self.semantic.resolves_to_local(name, name_span) {
            return;
        }
        // A writable base (`_G.x = 1`, `shared.score = 1` on Roblox) is
        // the deliberate global-set idiom, and so is assigning a leaf
        // the model marks writable (`package.path = ...`).
        if !is_overwritable_builtin(self.semantic, name) {
            return;
        }
        if let Some(segments) = var_dotted_segments(var)
            && let Some(entry) = self
                .semantic
                .lookup_stdlib_str(&segments.iter().map(String::as_str).collect::<Vec<_>>())
            && !entry.is_read_only()
        {
            return;
        }
        let display = var_dotted_path(var).unwrap_or_else(|| name.to_string());
        self.diagnostics.push(diagnostic(&display, span));
    }

    fn check_func_name(&mut self, func_name: &FuncName) {
        // Bare `function print()` surfaces as an unresolved Write reference.
        if func_name.names.len() < 2 && func_name.method.is_none() {
            return;
        }
        let Some(first) = func_name.names.first() else {
            return;
        };
        let TokenKind::Identifier(name) = &first.kind else {
            return;
        };
        if self.semantic.resolves_to_local(name, first.span) {
            return;
        }
        if !is_overwritable_builtin(self.semantic, name) {
            return;
        }
        let mut display = name.to_string();
        for token in &func_name.names[1..] {
            if let TokenKind::Identifier(part) = &token.kind {
                display.push('.');
                display.push_str(part);
            }
        }
        if let Some(method) = &func_name.method
            && let TokenKind::Identifier(part) = &method.kind
        {
            display.push(':');
            display.push_str(part);
        }
        self.diagnostics.push(diagnostic(&display, func_name.span));
    }
}

impl<'ast> Visitor<'ast> for FieldWriteChecker<'_> {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Assignment(assign) => {
                for var in assign.targets.iter() {
                    self.check_var_target(var);
                }
            }
            Statement::CompoundAssignment(compound) => self.check_var_target(&compound.var),
            Statement::FunctionDecl(decl) => self.check_func_name(&decl.name),
            _ => {}
        }
        self.walk_statement(stmt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&BuiltinGlobalWrite, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_assignment_to_builtin() {
        let diags = run("print = 5");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`print`"));
    }

    #[test]
    fn flags_global_function_decl_over_builtin() {
        let diags = run("function print() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`print`"));
    }

    #[test]
    fn flags_dotted_function_decl_on_builtin() {
        let diags = run("function table.foo() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`table.foo`"));
    }

    #[test]
    fn flags_method_decl_on_builtin() {
        let diags = run("function string:shout() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`string:shout`"));
    }

    #[test]
    fn flags_field_write_on_builtin() {
        let diags = run("table.insert = function() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`table.insert`"));
    }

    #[test]
    fn flags_index_write_on_builtin() {
        let diags = run("string[\"format\"] = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`string`"));
    }

    #[test]
    fn flags_nested_field_write_on_builtin() {
        let diags = run("do math.pi = 3 end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`math.pi`"));
    }

    #[test]
    fn ignores_shadowed_builtin() {
        let diags = run("local print = 1\nprint = 5");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_field_write_on_shadowed_builtin() {
        let diags = run("local table = {}\ntable.insert = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_function_decl_on_shadowed_builtin() {
        let diags = run("local table = {}\nfunction table.foo() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_g_field_writes() {
        let diags = run("_G.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_writable_leaf_property() {
        // The model marks package.path read-write; assigning it is the
        // documented configuration idiom, not an overwrite.
        let diags = run("package.path = package.path .. ';./?.lua'");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_writable_global_assignment() {
        // _PROMPT is a read-write interpreter property; setting it is
        // its entire purpose.
        let diags = run("_PROMPT = '> '");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_shared_field_write_on_roblox() {
        let diags = crate::test_support::run_rule_roblox(&BuiltinGlobalWrite, "shared.score = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_readonly_leaf_beside_writable_sibling() {
        // package.path is writable but math.huge is not; only
        // model-writable targets are exempt.
        let diags = run("math.huge = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_reads() {
        let diags = run("print(table.concat({}))");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_plain_global_write() {
        // setting_global owns non-builtin implicit globals.
        let diags = run("my_module_state = 5");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_extra_global_write() {
        let mut config = crate::LintConfig::default();
        config.extra_globals.push("vim".to_string());
        let diags = crate::test_support::run_rule_with_config(
            &BuiltinGlobalWrite,
            "vim = 5\nvim.api = 1",
            LuaVersion::Lua54,
            &config,
        );
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
