use luck_ast::{AstKind, AstType, FunctionName, Statement};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::Span;
use rustc_hash::FxHashMap;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DuplicateFunction;

declare_lint_rule!(
    DuplicateFunction,
    Suspicious,
    Warning,
    "function with the same name is defined twice in the same block"
);

impl Rule for DuplicateFunction {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::Block(block) = node.kind() else {
            return;
        };
        let mut definitions: FxHashMap<String, Span> = FxHashMap::default();
        for stmt in &block.stmts {
            // `redefining_local` covers local functions.
            let (declared_name, name_span) = if let Statement::FunctionDecl(decl) = stmt {
                (full_name(&decl.name), decl.name.span)
            } else if let Statement::GlobalFunction(function) = stmt {
                // Lua 5.5
                (Some(function.name.text.as_str().to_owned()), function.name.span)
            } else {
                continue;
            };
            let Some(name) = declared_name else {
                continue;
            };
            if let Some(&first_span) = definitions.get(&name) {
                context.diagnostic(
                    Finding::new(format!("duplicate function definition `{name}`"), name_span)
                        .with_label(first_span, "first defined here"),
                );
            } else {
                definitions.insert(name, name_span);
            }
        }
    }
}

/// The declared name as written, such as `a.b:c`.
fn full_name(function_name: &FunctionName<'_>) -> Option<String> {
    let mut names = function_name.names.iter();
    let mut joined = names.next()?.text.as_str().to_owned();
    for name in names {
        joined.push('.');
        joined.push_str(&name.text);
    }
    if let Some(method) = &function_name.method {
        joined.push(':');
        joined.push_str(&method.text);
    }
    Some(joined)
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::{labeled, run_rule};
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::DuplicateFunction, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_duplicate_plain_function() {
        let source = "\
function f() end
function f() end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].message, "duplicate function definition `f`");
        assert_eq!(diags[0].span().start, 26);
        assert_eq!(labeled(&diags[0], source), ["f"]);
        assert_eq!(diags[0].labels[0].span.start, 9);
    }

    #[test]
    fn flags_duplicate_dotted_function() {
        let source = "\
local t = {}
function t.m() end
function t.m() end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`t.m`"));
        assert_eq!(labeled(&diags[0], source), ["t.m"]);
    }

    #[test]
    fn flags_duplicate_method() {
        let diags = run("\
local t = {}
function t:m() end
function t:m() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`t:m`"));
    }

    #[test]
    fn flags_duplicates_in_nested_block() {
        let diags = run("\
do
function f() end
function f() end
end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert_eq!(diags[0].labels.first().map(|label| label.span.start), Some(12));
    }

    #[test]
    fn flags_each_later_duplicate() {
        let diags = run("\
function f() end
function f() end
function f() end");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }

    #[test]
    fn ignores_same_name_with_dot_and_colon() {
        let diags = run("\
local t = {}
function t.m() end
function t:m() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_different_names() {
        let diags = run("\
function f() end
function g() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_local_functions() {
        let diags = run("\
local function f() end
local function f() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_sibling_blocks() {
        let diags = run("\
do function f() end end
do function f() end end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_duplicate_global_function() {
        let source = "\
global function f() end
global function f() end";
        let diags = run_rule(RuleId::DuplicateFunction, source, LuaVersion::Lua55);
        assert_eq!(diags.len(), 1, "both statements define `f` in one block: {diags:?}");
    }

    #[test]
    fn flags_each_repeat_in_a_long_block() {
        let mut functions = String::new();
        for i in 0..2_000u32 {
            let _unused = writeln!(functions, "function m.f{i}() end");
        }
        let diags = run(&format!("{functions}{functions}"));
        assert_eq!(diags.len(), 2_000, "{diags:?}");
    }
}
