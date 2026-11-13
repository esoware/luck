use luck_ast::{DottedPath, Expression, FunctionName, Statement, Var, Visitor, dotted_path};
use luck_semantic::{SemanticAnalysis, StdlibEntry};
use luck_token::{Span, Token};

use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct BuiltinGlobalWrite;

impl_rule!(
    BuiltinGlobalWrite,
    "builtin_global_write",
    Correctness,
    Warning,
    "overwriting a standard-library global or one of its fields"
);

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

struct FieldWriteChecker<'semantic> {
    semantic: &'semantic SemanticAnalysis,
    findings: Vec<Finding>,
}

impl FieldWriteChecker<'_> {
    /// A field of a stdlib value, such as `package.loaded` or a Roblox
    /// instance's property, isn't the library. Neither is a `path` that no
    /// longer reaches the host's global.
    fn writes_namespace_member(&self, path: &[&str], root_span: Span, leaf: Option<&str>) -> bool {
        let Some(&root) = path.first() else {
            return false;
        };
        // Writing through a writable base (`_G.x = 1`, Roblox's
        // `shared.score = 1`) is intended.
        if !is_overwritable_builtin(self.semantic, root)
            || !self.semantic.reaches_host_global(root, root_span)
        {
            return false;
        }
        let Some(StdlibEntry::Namespace(namespace)) = self.semantic.stdlib().lookup(path) else {
            return false;
        };
        // So is a leaf the model marks writable (`package.path = ...`).
        leaf.and_then(|name| namespace.members.get(name)).is_none_or(StdlibEntry::is_read_only)
    }

    fn check_var_target(&mut self, var: &Var) {
        let (prefix, leaf, span) = match var {
            // Whole-name writes surface as unresolved Write references.
            Var::Name(_) => return,
            Var::Index(index) => (&index.prefix, None, index.span),
            Var::FieldAccess(field_access) => {
                (&field_access.prefix, field_access.name.identifier(), field_access.span)
            }
        };
        let Expression::Var(container) = prefix else {
            return;
        };
        let Some(DottedPath { segments, root_span, .. }) = dotted_path(container) else {
            return;
        };
        if !self.writes_namespace_member(&segments, root_span, leaf) {
            return;
        }
        let mut display = segments.join(".");
        if let Some(name) = leaf {
            display.push('.');
            display.push_str(name);
        }
        self.findings.push(overwrite_finding(&display, span));
    }

    fn check_function_name(&mut self, function_name: &FunctionName) {
        let (container, leaf, separator) = if let Some(method) = &function_name.method {
            (function_name.names.as_slice(), method, ':')
        } else {
            // Bare `function print()` surfaces as an unresolved Write
            // reference, and has no container here.
            let Some((leaf, container)) = function_name.names.split_last() else {
                return;
            };
            (container, leaf, '.')
        };
        let Some(segments) = container.iter().map(Token::identifier).collect::<Option<Vec<_>>>()
        else {
            return;
        };
        let (Some(root), Some(leaf_name)) = (container.first(), leaf.identifier()) else {
            return;
        };
        if !self.writes_namespace_member(&segments, root.span, Some(leaf_name)) {
            return;
        }
        let display = format!("{}{separator}{leaf_name}", segments.join("."));
        self.findings.push(overwrite_finding(&display, function_name.span));
    }
}

impl<'ast> Visitor<'ast> for FieldWriteChecker<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
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
        self.walk_stmt(stmt);
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::config::LintConfig;
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
        let config = LintConfig { extra_globals: vec!["vim".to_owned()], ..LintConfig::default() };
        let diags = run_rule_with_config(
            &BuiltinGlobalWrite,
            "vim = 5\nvim.api = 1",
            LuaVersion::Lua54,
            &config,
        );
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_builtin_listed_in_extra_globals() {
        // `table` is a stdlib name, so only the `extra_globals` entry keeps
        // these writes quiet.
        let config =
            LintConfig { extra_globals: vec!["table".to_owned()], ..LintConfig::default() };
        let diags = run_rule_with_config(
            &BuiltinGlobalWrite,
            "table = 5\ntable.insert = 1\nfunction table.foo() end",
            LuaVersion::Lua54,
            &config,
        );
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_package_preload_entry_write() {
        let diags = run("package.preload.foo = function() end");
        assert!(diags.is_empty(), "`package.preload` exists to be filled: {diags:?}");
    }

    #[test]
    fn ignores_package_loaded_entry_write() {
        let diags = run("package.loaded[name] = nil");
        assert!(diags.is_empty(), "clearing a loaded module is documented: {diags:?}");
    }

    #[test]
    fn ignores_roblox_instance_property_write() {
        let diags = run_rule_roblox(&BuiltinGlobalWrite, "script.Parent.Visible = false");
        assert!(diags.is_empty(), "this sets a property, not a builtin: {diags:?}");
    }

    #[test]
    fn flags_only_the_whole_name_write_of_a_rebound_builtin() {
        let diags = run("table = {}\ntable.insert = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`table`"), "got: {diags:?}");
    }

    #[test]
    fn ignores_field_write_under_a_local_env() {
        let diags = run("local _ENV = {}\ntable.insert = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_function_decl_on_builtin_value() {
        let diags = run("function package.preload.foo() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
