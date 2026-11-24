use luck_ast::{AstKind, AstType, FunctionName, Name, Suffix, Var};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::{SemanticAnalysis, StdlibEntry};
use luck_token::Span;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BuiltinGlobalWrite;

declare_lint_rule!(
    BuiltinGlobalWrite,
    Correctness,
    Warning,
    "overwriting a standard-library global or one of its fields"
);

/// Every write of a name or a field is an assignment's, a compound
/// assignment's, or a `function` statement's.
impl Rule for BuiltinGlobalWrite {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[
        AstType::Assignment,
        AstType::CompoundAssignment,
        AstType::FunctionDecl,
    ]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let kind = node.kind();
        if let AstKind::Assignment(assignment) = kind {
            for var in &assignment.targets {
                check_var_target(var, context);
            }
            return;
        }
        // Luau
        if let AstKind::CompoundAssignment(compound) = kind {
            return check_var_target(&compound.var, context);
        }
        if let AstKind::FunctionDecl(decl) = kind {
            check_function_name(&decl.name, context);
        }
    }
}

/// Names in the config's extra globals belong to the user, even when they
/// collide with a stdlib name. Globals the stdlib model marks writable
/// (`_G`, Roblox's `shared`, `_PROMPT`) exist to be set.
fn is_overwritable_builtin(semantic: &SemanticAnalysis<'_>, name: &str) -> bool {
    semantic.is_read_only_global(name)
        && semantic.scoping().name_id(name).is_none_or(|id| !semantic.is_extra_global(id))
}

fn overwrite_finding(name: &str, span: Span) -> Finding {
    Finding::new(
        format!(
            "built-in global `{name}` is overwritten here, consider a local or a different name"
        ),
        span,
    )
}

/// A write of the whole global `name`.
fn check_name_write(name: &Name<'_>, context: &LintContext<'_>) {
    let tree = context.scoping();
    if let Some(reference) = tree.reference_of(name).map(|id| tree.reference(id))
        && reference.kind.is_write()
        && tree.is_global(reference)
        && is_overwritable_builtin(context.semantic, name.text.as_str())
    {
        context.diagnostic(overwrite_finding(name.text.as_str(), reference.span));
    }
}

/// A field of a stdlib value, such as `package.loaded` or a Roblox
/// instance's property, isn't the library. Neither is a `path` that no
/// longer reaches the host's global.
fn writes_namespace_member<'path>(
    semantic: &SemanticAnalysis<'_>,
    root: &Name<'_>,
    path: impl IntoIterator<Item = &'path str>,
    leaf: Option<&str>,
) -> bool {
    // Writing through a writable base (`_G.x = 1`, Roblox's
    // `shared.score = 1`) is intended.
    if !is_overwritable_builtin(semantic, root.text.as_str())
        || !semantic.scoping().reaches_host_global(root)
    {
        return false;
    }
    let Some(StdlibEntry::Namespace(namespace)) = semantic.stdlib().lookup(path) else {
        return false;
    };
    // So is a leaf the model marks writable (`package.path = ...`).
    leaf.and_then(|name| namespace.members.get(name)).is_none_or(StdlibEntry::is_read_only)
}

fn check_var_target(var: &Var<'_>, context: &LintContext<'_>) {
    let chain = match var {
        Var::Name(name) => return check_name_write(name, context),
        Var::Suffixed(chain) => chain,
    };
    let Some((container, last)) = chain.head(chain.suffixes.len()).split_last() else {
        return;
    };
    let leaf = match last {
        Suffix::Field(name) => Some(name.text.as_str()),
        Suffix::Index { .. } => None,
        Suffix::Call(_) | Suffix::MethodCall(_) | Suffix::TypeInstantiation { .. } => return,
    };
    let Some(path) = container.dotted_path() else {
        return;
    };
    if !writes_namespace_member(context.semantic, path.root(), path.segments(), leaf) {
        return;
    }
    let mut display = path.to_string();
    if let Some(name) = leaf {
        display.push('.');
        display.push_str(name);
    }
    context.diagnostic(overwrite_finding(&display, chain.span));
}

fn check_function_name(function_name: &FunctionName<'_>, context: &LintContext<'_>) {
    let (container, leaf, separator) = if let Some(method) = &function_name.method {
        (&*function_name.names, method, ':')
    } else {
        let Some((leaf, container)) = function_name.names.split_last() else {
            return;
        };
        if container.is_empty() {
            return check_name_write(leaf, context);
        }
        (container, leaf, '.')
    };
    let segments: Vec<&str> = container.iter().map(|name| name.text.as_str()).collect();
    let Some(root) = container.first() else {
        return;
    };
    let leaf_name = leaf.text;
    if !writes_namespace_member(
        context.semantic,
        root,
        segments.iter().copied(),
        Some(leaf_name.as_str()),
    ) {
        return;
    }
    let display = format!("{}{separator}{leaf_name}", segments.join("."));
    context.diagnostic(overwrite_finding(&display, function_name.span));
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::config::LintConfig;
    use crate::diagnostic::Message;
    use crate::fixtures::{run_rule, run_rule_roblox, run_rule_with_config};
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::BuiltinGlobalWrite, source, LuaVersion::Lua54)
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
        let diags = run("\
local print = 1
print = 5");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_field_write_on_shadowed_builtin() {
        let diags = run("\
local table = {}
table.insert = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_function_decl_on_shadowed_builtin() {
        let diags = run("\
local table = {}
function table.foo() end");
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
        let diags = run_rule_roblox(RuleId::BuiltinGlobalWrite, "shared.score = 1");
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
            RuleId::BuiltinGlobalWrite,
            "\
vim = 5
vim.api = 1",
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
            RuleId::BuiltinGlobalWrite,
            "\
table = 5
table.insert = 1
function table.foo() end",
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
        let diags = run_rule_roblox(RuleId::BuiltinGlobalWrite, "script.Parent.Visible = false");
        assert!(diags.is_empty(), "this sets a property, not a builtin: {diags:?}");
    }

    #[test]
    fn flags_only_the_whole_name_write_of_a_rebound_builtin() {
        let diags = run("\
table = {}
table.insert = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`table`"), "got: {diags:?}");
    }

    #[test]
    fn ignores_field_write_under_a_local_env() {
        let diags = run("\
local _ENV = {}
table.insert = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_function_decl_on_builtin_value() {
        let diags = run("function package.preload.foo() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
