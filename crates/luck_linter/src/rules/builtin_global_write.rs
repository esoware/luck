use luck_ast::{AstKind, AstType, FunctionName, Name, Suffix, Var};
use luck_semantic::{AstNode, AstTypesBitset, Reference};
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

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let kind = node.kind();
        if let AstKind::Assignment(assignment) = kind {
            for var in &assignment.targets {
                check_var_target(var, context);
            }
            return;
        }
        if let AstKind::CompoundAssignment(compound) = kind {
            return check_var_target(&compound.var, context);
        }
        if let AstKind::FunctionDecl(decl) = kind {
            check_function_name(&decl.name, context);
        }
    }
}

/// Whether the global `reference` names is a builtin no code should write,
/// which every global the library declares is.
///
/// Names in the config's extra globals belong to the user, even when they
/// collide with a stdlib name.
fn is_overwritable_builtin(semantic: &SemanticAnalysis<'_>, reference: &Reference) -> bool {
    semantic.is_read_only_global(semantic.scoping().name(reference.name))
        && !semantic.is_extra_global(reference.name)
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
fn check_name_write(name: &Name<'_>, context: &LintContext<'_, '_>) {
    let tree = context.scoping();
    if let Some(reference) = tree.reference_of(name).map(|id| tree.reference(id))
        && reference.kind.is_write()
        && reference.is_global()
        && is_overwritable_builtin(context.semantic, reference)
    {
        context.diagnostic(overwrite_finding(name.text.as_str(), reference.span));
    }
}

/// A field of a stdlib value, such as a Roblox instance's property, isn't
/// the library. Neither is a `path` that no
/// longer reaches the host's global.
fn writes_namespace_member<'path>(
    semantic: &SemanticAnalysis<'_>,
    root: &Name<'_>,
    path: impl IntoIterator<Item = &'path str>,
    leaf: Option<&str>,
) -> bool {
    let tree = semantic.scoping();
    let Some(reference) = tree.reference_of(root).map(|id| tree.reference(id)) else {
        return false;
    };
    if !is_overwritable_builtin(semantic, reference) || !tree.is_host_global(reference) {
        return false;
    }
    let Some(StdlibEntry::Namespace(namespace)) = semantic.stdlib().lookup(path) else {
        return false;
    };
    // So is a field a definition doesn't declare `read`, as Luau types it.
    leaf.and_then(|name| namespace.members.get(name)).is_none_or(StdlibEntry::is_read_only)
}

fn check_var_target(var: &Var<'_>, context: &LintContext<'_, '_>) {
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

fn check_function_name(function_name: &FunctionName<'_>, context: &LintContext<'_, '_>) {
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
    let Some(root) = container.first() else {
        return;
    };
    let segments = container.iter().map(|name| name.text.as_str());
    if !writes_namespace_member(context.semantic, root, segments.clone(), Some(leaf.text.as_str()))
    {
        return;
    }
    let path: Vec<&str> = segments.collect();
    let display = format!("{}{separator}{}", path.join("."), leaf.text);
    context.diagnostic(overwrite_finding(&display, function_name.span));
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, Option<&str>, Option<&str>)] = &[
        (
            "\
local print = 1
print = 5",
            None,
            None,
        ),
        (
            "\
local table = {}
table.insert = 1",
            None,
            None,
        ),
        (
            "\
local table = {}
function table.foo() end",
            None,
            None,
        ),
        ("_G.foo = 1", None, None),
        ("print(table.concat({}))", None, None),
        // A library's fields are writable unless declared `read`.
        ("table.insert = function() end", None, None),
        ("do math.pi = 3 end", None, None),
        // `setting_global` covers globals that aren't builtins.
        ("my_module_state = 5", None, None),
        (
            "\
app = 5
app.api = 1",
            None,
            Some(r#"{"extra_globals": ["app"]}"#),
        ),
        // `table` is a stdlib name, so only the `extra_globals` entry keeps these
        // writes quiet.
        (
            "\
table = 5
table.insert = 1
function table.foo() end",
            None,
            Some(r#"{"extra_globals": ["table"]}"#),
        ),
    ];

    const FAIL: &[(&str, Option<&str>, Option<&str>)] = &[
        ("print = 5", None, None),
        ("function print() end", None, None),
        ("function table.foo() end", None, None),
        ("function string:shout() end", None, None),
        (r#"string["format"] = 1"#, None, None),
        ("_G = {}", None, None),
        (
            "\
table = {}
table.insert = 1",
            None,
            None,
        ),
    ];

    const ROBLOX_PASS: &[&str] = &[
        "shared.score = 1",
        // This sets a property, not a builtin.
        "script.Parent.Visible = false",
    ];

    const DEFINITIONS: &str = "declare app: { read version: string, name: string }";

    const DEFINITIONS_PASS: &[&str] = &["app.name = 'x'"];

    const DEFINITIONS_FAIL: &[&str] = &["app.version = '2'", "app = {}"];

    #[test]
    fn test() {
        Tester::new(RuleId::BuiltinGlobalWrite, PASS, FAIL)
            .with_roblox(ROBLOX_PASS, &[])
            .with_definitions(DEFINITIONS, DEFINITIONS_PASS, DEFINITIONS_FAIL)
            .test_and_snapshot();
    }
}
