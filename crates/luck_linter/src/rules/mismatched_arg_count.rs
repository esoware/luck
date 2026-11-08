use luck_ast::{Expression, FunctionBody, FunctionCall, Statement, Var, Visitor};
use luck_semantic::{ReferenceKind, ScopeTree, SymbolId};
use luck_token::Span;
use rustc_hash::FxHashMap;

use crate::call::{has_multi_value_tail, positional_args};
use crate::diagnostic::{Category, LintDiagnostic, Severity, line_column};
use crate::rule::{LintContext, Rule, WholeRule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct MismatchedArgCount;

impl Rule for MismatchedArgCount {
    fn name(&self) -> &'static str {
        "mismatched_arg_count"
    }

    fn category(&self) -> Category {
        Category::Correctness
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "call passes more arguments than the called function accepts"
    }
}

impl WholeRule for MismatchedArgCount {
    fn check(&self, context: &LintContext<'_>) -> Vec<LintDiagnostic> {
        let tree = &context.semantic.scope_tree;
        let mut definitions = DefinitionCollector { tree, functions: FxHashMap::default() };
        definitions.visit_block(context.block);
        if definitions.functions.is_empty() {
            return Vec::new();
        }
        let mut checker = CallChecker {
            tree,
            source: context.source,
            functions: definitions.functions,
            diags: Vec::new(),
        };
        checker.visit_block(context.block);
        checker.diags
    }
}

struct FunctionSignature {
    param_count: usize,
    has_vararg: bool,
    definition_start: u32,
}

/// Maps each local bound to a function literal to that function's signature.
/// A local that's ever reassigned is left out, since it could hold any
/// function by the time a call runs.
struct DefinitionCollector<'tree> {
    tree: &'tree ScopeTree,
    functions: FxHashMap<SymbolId, FunctionSignature>,
}

impl DefinitionCollector<'_> {
    fn record(&mut self, name_span: Span, body: &FunctionBody) {
        let Some(symbol) =
            self.tree.symbols.iter().find(|symbol| symbol.definition_span == name_span)
        else {
            return;
        };
        let is_reassigned = symbol.reference_ids.iter().any(|&reference_id| {
            matches!(
                self.tree.reference(reference_id).kind,
                ReferenceKind::Write | ReferenceKind::ReadWrite
            )
        });
        if is_reassigned {
            return;
        }
        self.functions.insert(
            symbol.id,
            FunctionSignature {
                param_count: body.params.len(),
                has_vararg: body.vararg.is_some(),
                definition_start: name_span.start,
            },
        );
    }
}

impl<'ast> Visitor<'ast> for DefinitionCollector<'_> {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalFunction(local_function) => {
                self.record(local_function.name.span, &local_function.body);
            }
            Statement::LocalAssignment(local) => {
                if let Some(exprs) = &local.exprs {
                    for (attributed, value) in local.names.iter().zip(exprs.iter()) {
                        if let Expression::FunctionDef(function) = value {
                            self.record(attributed.name.span, &function.body);
                        }
                    }
                }
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_statement(stmt);
    }
}

struct CallChecker<'context> {
    tree: &'context ScopeTree,
    source: &'context str,
    functions: FxHashMap<SymbolId, FunctionSignature>,
    diags: Vec<LintDiagnostic>,
}

impl CallChecker<'_> {
    fn check_call(&mut self, call: &FunctionCall) {
        if call.method.is_some() {
            return;
        }
        let Expression::Var(Var::Name(token)) = &call.callee else {
            return;
        };
        let Some(name) = token.identifier() else {
            return;
        };
        let Some(signature) = self.resolve(token.span) else {
            return;
        };
        if signature.has_vararg {
            return;
        }
        let (args, arg_count) = positional_args(&call.args);
        // A trailing call or `...` can expand to nothing, so only the args
        // before it are sure to be passed.
        let guaranteed_arg_count =
            arg_count.saturating_sub(usize::from(has_multi_value_tail(args)));
        if guaranteed_arg_count <= signature.param_count {
            return;
        }
        let (line, _) = line_column(self.source, signature.definition_start);
        self.diags.push(
            LintDiagnostic::new(
                "mismatched_arg_count",
                format!(
                    "call passes {guaranteed_arg_count} arguments to function `{name}`, \
                     which accepts only {} (defined on line {line})",
                    signature.param_count
                ),
                call.span,
            )
            .with_help("extra arguments are silently discarded"),
        );
    }

    /// The signature of the local function the name at `span` calls.
    fn resolve(&self, span: Span) -> Option<&FunctionSignature> {
        let reference = self.tree.references.iter().find(|reference| reference.span == span)?;
        self.functions.get(&reference.resolved?)
    }
}

impl<'ast> Visitor<'ast> for CallChecker<'_> {
    fn visit_function_call(&mut self, call: &'ast FunctionCall) {
        self.check_call(call);
        self.walk_function_call(call);
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::MismatchedArgCount;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&MismatchedArgCount, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_extra_args_to_local_function() {
        let diags = run("local function f(a, b) return a + b end\nf(1, 2, 3)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("accepts only 2"), "{diags:?}");
    }

    #[test]
    fn flags_extra_args_to_function_expression() {
        let diags = run("local f = function(a) return a end\nf(1, 2)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_zero_param_function() {
        let diags = run("local function f() end\nf(1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_guaranteed_overflow_with_trailing_call() {
        // g() may yield nothing, but 1 and 2 already exceed one param.
        let diags = run("local function g() end\nlocal function f(a) return a end\nf(1, 2, g())");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_exact_arg_count() {
        let diags = run("local function f(a, b) return a + b end\nf(1, 2)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_fewer_args() {
        // Missing args are nil, which is how Lua does optional arguments.
        let diags = run("local function f(a, b) return a or b end\nf(1)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_vararg_function() {
        let diags = run("local function f(...) return ... end\nf(1, 2, 3, 4)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_trailing_call_at_boundary() {
        // f(1, g()): g() may yield zero values, so 1 arg is guaranteed.
        let diags = run("local function g() end\nlocal function f(a) return a end\nf(1, g())");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_reassigned_function() {
        let diags =
            run("local function f(a) return a end\nf = function(a, b, c) return a end\nf(1, 2, 3)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_global_function_calls() {
        let diags = run("print(1, 2, 3)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_table_field_calls() {
        let diags = run("local t = { f = function(a) return a end }\nt.f(1, 2)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_recursive_call_within_limit() {
        let diags = run("local function f(a)\n    if a > 0 then return f(a - 1) end\nend\nf(3)");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
