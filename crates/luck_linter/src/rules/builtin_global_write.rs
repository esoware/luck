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
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget, Option<&str>, Option<&str>)] = &[
        (
            "\
local print = 1
print = 5",
            LuaTarget::Lua54,
            None,
            None,
        ),
        (
            "\
local table = {}
table.insert = 1",
            LuaTarget::Lua54,
            None,
            None,
        ),
        (
            "\
local table = {}
function table.foo() end",
            LuaTarget::Lua54,
            None,
            None,
        ),
        ("_G.foo = 1", LuaTarget::Lua54, None, None),
        // The model marks `package.path` writable.
        ("package.path = package.path .. ';./?.lua'", LuaTarget::Lua54, None, None),
        // `_PROMPT` exists to be set.
        ("_PROMPT = '> '", LuaTarget::Lua54, None, None),
        ("shared.score = 1", LuaTarget::LuauRoblox, None, None),
        ("print(table.concat({}))", LuaTarget::Lua54, None, None),
        // `setting_global` covers globals that aren't builtins.
        ("my_module_state = 5", LuaTarget::Lua54, None, None),
        (
            "\
vim = 5
vim.api = 1",
            LuaTarget::Lua54,
            None,
            Some(r#"{"extra_globals": ["vim"]}"#),
        ),
        // `table` is a stdlib name, so only the `extra_globals` entry keeps these
        // writes quiet.
        (
            "\
table = 5
table.insert = 1
function table.foo() end",
            LuaTarget::Lua54,
            None,
            Some(r#"{"extra_globals": ["table"]}"#),
        ),
        // `package.preload` exists to be filled.
        ("package.preload.foo = function() end", LuaTarget::Lua54, None, None),
        // Clearing a loaded module is documented.
        ("package.loaded[name] = nil", LuaTarget::Lua54, None, None),
        // This sets a property, not a builtin.
        ("script.Parent.Visible = false", LuaTarget::LuauRoblox, None, None),
        (
            "\
local _ENV = {}
table.insert = 1",
            LuaTarget::Lua54,
            None,
            None,
        ),
        ("function package.preload.foo() end", LuaTarget::Lua54, None, None),
    ];

    const FAIL: &[(&str, LuaTarget, Option<&str>, Option<&str>)] = &[
        ("print = 5", LuaTarget::Lua54, None, None),
        ("function print() end", LuaTarget::Lua54, None, None),
        ("function table.foo() end", LuaTarget::Lua54, None, None),
        ("function string:shout() end", LuaTarget::Lua54, None, None),
        ("table.insert = function() end", LuaTarget::Lua54, None, None),
        (r#"string["format"] = 1"#, LuaTarget::Lua54, None, None),
        ("do math.pi = 3 end", LuaTarget::Lua54, None, None),
        // Only leaves the model marks writable, like `package.path`, are exempt.
        ("math.huge = 1", LuaTarget::Lua54, None, None),
        (
            "\
table = {}
table.insert = 1",
            LuaTarget::Lua54,
            None,
            None,
        ),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::BuiltinGlobalWrite, PASS, FAIL).test_and_snapshot();
    }
}
