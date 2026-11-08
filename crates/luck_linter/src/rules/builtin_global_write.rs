use luck_ast::{Expression, FunctionName, Statement, Var, Visitor};
use luck_semantic::SemanticAnalysis;
use luck_token::{Span, Token};

use crate::diagnostic::{Category, Finding, Severity};
use crate::path::dotted_path;
use crate::rule::{LintContext, Rule, WholeRule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct BuiltinGlobalWrite;

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
}

impl WholeRule for BuiltinGlobalWrite {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let mut findings = Vec::new();

        for reference in semantic.scope_tree.unresolved_references() {
            if !reference.kind.is_write() {
                continue;
            }
            if !is_overwritable_builtin(semantic, &reference.name) {
                continue;
            }
            findings.push(overwrite_finding(&reference.name, reference.span));
        }

        let mut checker = FieldWriteChecker { semantic, findings };
        checker.visit_block(context.block);
        checker.findings
    }
}

/// Whether writing to `name` overwrites a read-only builtin.
///
/// Names in the config's extra globals belong to the user, even when they
/// collide with a stdlib name. Globals the stdlib model marks writable
/// (`_G`, Roblox's `shared`, `_PROMPT`) exist to be set.
fn is_overwritable_builtin(semantic: &SemanticAnalysis, name: &str) -> bool {
    semantic.is_known_global(name)
        && semantic.is_read_only_global(name)
        && !semantic.extra_globals.contains(name)
}

fn overwrite_finding(name: &str, span: Span) -> Finding {
    Finding::new(
        format!(
            "built-in global `{name}` is overwritten here, consider a local or a different name"
        ),
        span,
    )
}

fn expr_base_identifier(expr: &Expression) -> Option<(&str, Span)> {
    let Expression::Var(var) = expr else {
        return None;
    };
    match var {
        Var::Name(token) => Some((token.identifier()?, token.span)),
        Var::FieldAccess(field_access) => expr_base_identifier(&field_access.prefix),
        Var::Index(index) => expr_base_identifier(&index.prefix),
    }
}

struct FieldWriteChecker<'semantic> {
    semantic: &'semantic SemanticAnalysis,
    findings: Vec<Finding>,
}

impl FieldWriteChecker<'_> {
    fn check_var_target(&mut self, var: &Var) {
        let (base, span) = match var {
            // Whole-name writes surface as unresolved Write references.
            Var::Name(_) => return,
            Var::Index(index) => (expr_base_identifier(&index.prefix), index.span),
            Var::FieldAccess(field_access) => {
                (expr_base_identifier(&field_access.prefix), field_access.span)
            }
        };
        let Some((name, name_span)) = base else {
            return;
        };
        if self.semantic.resolves_to_local(name, name_span) {
            return;
        }
        // Writing through a writable base (`_G.x = 1`, Roblox's
        // `shared.score = 1`) or to a leaf the model marks writable
        // (`package.path = ...`) is intended.
        if !is_overwritable_builtin(self.semantic, name) {
            return;
        }
        let path_segments = dotted_path(var).map(|(segments, _)| segments);
        if let Some(segments) = &path_segments
            && let Some(entry) = self.semantic.stdlib().lookup(segments)
            && !entry.is_read_only()
        {
            return;
        }
        let display = path_segments.map_or_else(|| name.to_owned(), |segments| segments.join("."));
        self.findings.push(overwrite_finding(&display, span));
    }

    fn check_function_name(&mut self, function_name: &FunctionName) {
        // Bare `function print()` surfaces as an unresolved Write reference.
        if function_name.names.len() < 2 && function_name.method.is_none() {
            return;
        }
        let Some(first) = function_name.names.first() else {
            return;
        };
        let Some(name) = first.identifier() else {
            return;
        };
        if self.semantic.resolves_to_local(name, first.span) {
            return;
        }
        if !is_overwritable_builtin(self.semantic, name) {
            return;
        }
        let mut display = name.to_owned();
        for part in function_name.names.iter().skip(1).filter_map(Token::identifier) {
            display.push('.');
            display.push_str(part);
        }
        if let Some(part) = function_name.method.as_ref().and_then(Token::identifier) {
            display.push(':');
            display.push_str(part);
        }
        self.findings.push(overwrite_finding(&display, function_name.span));
    }
}

impl<'ast> Visitor<'ast> for FieldWriteChecker<'_> {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Assignment(assignment) => {
                for var in assignment.targets.iter() {
                    self.check_var_target(var);
                }
            }
            // Luau
            Statement::CompoundAssignment(compound) => self.check_var_target(&compound.var),
            Statement::FunctionDecl(decl) => self.check_function_name(&decl.name),
            Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
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
        self.walk_statement(stmt);
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{run_rule, run_rule_roblox, run_rule_with_config};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&BuiltinGlobalWrite, source, LuaVersion::Lua54)
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
        // The model marks `package.path` writable.
        let diags = run("package.path = package.path .. ';./?.lua'");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_writable_global_assignment() {
        // `_PROMPT` exists to be set.
        let diags = run("_PROMPT = '> '");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_shared_field_write_on_roblox() {
        let diags = run_rule_roblox(&BuiltinGlobalWrite, "shared.score = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_readonly_leaf_beside_writable_sibling() {
        // Only leaves the model marks writable, like `package.path`, are
        // exempt.
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
        // `setting_global` covers globals that aren't builtins.
        let diags = run("my_module_state = 5");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_extra_global_write() {
        let mut config = crate::LintConfig::default();
        config.extra_globals.push("vim".to_owned());
        let diags = run_rule_with_config(
            &BuiltinGlobalWrite,
            "vim = 5\nvim.api = 1",
            LuaVersion::Lua54,
            &config,
        );
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
