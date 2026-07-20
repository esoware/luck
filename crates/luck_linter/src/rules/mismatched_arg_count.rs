use std::collections::HashMap;

use luck_ast::Expression;
use luck_ast::expr::{FunctionArgs, FunctionCall, Var};
use luck_ast::shared::FunctionBody;
use luck_ast::stmt::Statement;
use luck_ast::visitor::Visitor;
use luck_semantic::scope::{ReferenceKind, SymbolId};
use luck_token::{Span, TokenKind};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, Rule};

pub struct MismatchedArgCount;

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
        "Call passes more arguments than the called function accepts."
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let mut definitions = DefinitionCollector {
            ctx,
            functions: HashMap::new(),
        };
        definitions.visit_block(ctx.block);
        if definitions.functions.is_empty() {
            return Vec::new();
        }
        let mut checker = CallChecker {
            ctx,
            functions: definitions.functions,
            diagnostics: Vec::new(),
        };
        checker.visit_block(ctx.block);
        checker.diagnostics
    }
}

struct FunctionSignature {
    param_count: usize,
    has_vararg: bool,
    definition_start: u32,
}

/// Maps local symbols to the function signature they are bound to.
/// Only single-write symbols qualify: a reassigned `f` could hold any
/// function by the time a call runs.
struct DefinitionCollector<'a> {
    ctx: &'a LintContext<'a>,
    functions: HashMap<SymbolId, FunctionSignature>,
}

impl DefinitionCollector<'_> {
    fn record(&mut self, name_span: Span, body: &FunctionBody) {
        let Some(symbol_id) = self.symbol_defined_at(name_span) else {
            return;
        };
        let tree = &self.ctx.semantic.scope_tree;
        let symbol = &tree.symbols[symbol_id.index()];
        let reassigned = symbol.reference_ids.iter().any(|&ref_id| {
            matches!(
                tree.references[ref_id.index()].kind,
                ReferenceKind::Write | ReferenceKind::ReadWrite
            )
        });
        if reassigned {
            return;
        }
        self.functions.insert(
            symbol_id,
            FunctionSignature {
                param_count: body.params.iter().count(),
                has_vararg: body.vararg.is_some(),
                definition_start: name_span.start,
            },
        );
    }

    fn symbol_defined_at(&self, span: Span) -> Option<SymbolId> {
        self.ctx
            .semantic
            .scope_tree
            .symbols
            .iter()
            .find(|symbol| symbol.definition_span == span)
            .map(|symbol| symbol.id)
    }
}

impl<'ast> Visitor<'ast> for DefinitionCollector<'_> {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalFunction(local_fn) => {
                self.record(local_fn.name.span, &local_fn.body);
            }
            Statement::LocalAssignment(local) => {
                if let Some(exprs) = &local.exprs {
                    for (attributed, value) in local.names.iter().zip(exprs.iter()) {
                        if let Expression::FunctionDef(def) = value {
                            self.record(attributed.name.span, &def.body);
                        }
                    }
                }
            }
            _ => {}
        }
        self.walk_statement(stmt);
    }
}

struct CallChecker<'a> {
    ctx: &'a LintContext<'a>,
    functions: HashMap<SymbolId, FunctionSignature>,
    diagnostics: Vec<LintDiagnostic>,
}

impl CallChecker<'_> {
    fn check_call(&mut self, call: &FunctionCall) {
        if call.method.is_some() {
            return;
        }
        let Expression::Var(var) = &call.callee else {
            return;
        };
        let Var::Name(token) = var else {
            return;
        };
        let TokenKind::Identifier(name) = &token.kind else {
            return;
        };
        let Some(signature) = self.resolve(token.span) else {
            return;
        };
        if signature.has_vararg {
            return;
        }
        let (arg_count, last_expands) = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                let exprs: Vec<&Expression> = args.iter().collect();
                let last_expands = matches!(
                    exprs.last(),
                    Some(Expression::FunctionCall(_)) | Some(Expression::VarArg(_))
                );
                (exprs.len(), last_expands)
            }
            FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => (1, false),
        };
        // A trailing call/vararg can expand to zero values, so only the
        // guaranteed-present preceding args can prove an overflow.
        let guaranteed = if last_expands {
            arg_count - 1
        } else {
            arg_count
        };
        if guaranteed <= signature.param_count {
            return;
        }
        let line = line_number(self.ctx.source, signature.definition_start);
        self.diagnostics.push(
            LintDiagnostic::new(
                "mismatched_arg_count",
                format!(
                    "call passes {guaranteed} arguments to function '{name}', which accepts only \
                     {} (defined on line {line})",
                    signature.param_count
                ),
                call.span,
            )
            .with_help("extra arguments are silently discarded".to_string()),
        );
    }

    fn resolve(&self, span: Span) -> Option<&FunctionSignature> {
        let tree = &self.ctx.semantic.scope_tree;
        let reference = tree
            .references
            .iter()
            .find(|reference| reference.span == span)?;
        self.functions.get(&reference.resolved?)
    }
}

impl<'ast> Visitor<'ast> for CallChecker<'_> {
    fn visit_expression(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.check_call(call);
        }
        self.walk_expression(expr);
    }

    fn visit_statement(&mut self, stmt: &'ast Statement) {
        if let Statement::FunctionCall(call_stmt) = stmt {
            self.check_call(&call_stmt.call);
        }
        self.walk_statement(stmt);
    }
}

fn line_number(source: &str, offset: u32) -> usize {
    source[..offset as usize]
        .bytes()
        .filter(|&b| b == b'\n')
        .count()
        + 1
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::MismatchedArgCount;
    use crate::diagnostic::LintDiagnostic;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&MismatchedArgCount, source, LuaVersion::Lua54)
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
        // Missing args default to nil; that is idiomatic optional-arg Lua.
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
