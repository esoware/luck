use luck_ast::{Expression, Statement, Var, Visitor};
use luck_semantic::ReferenceKind;

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, Rule, WholeRule};
use crate::uninitialized::{UninitializedLocals, first_reference};

/// Flags a field or index write on a local declared without a value, before
/// anything assigns the local itself (Luacheck 341). `local x; x.foo = 1`
/// indexes `nil`.
///
/// The scope tree records the `x` in `x.foo = ...` as a plain read, so
/// `MutationCollector` finds the reads that are write prefixes to tell them
/// apart from `print(x)`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MutatingUninitialized;

impl Rule for MutatingUninitialized {
    fn name(&self) -> &'static str {
        "mutating_uninitialized"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "field or index assignment on an uninitialized local"
    }
}

impl WholeRule for MutatingUninitialized {
    fn check(&self, context: &LintContext<'_>) -> Vec<LintDiagnostic> {
        let tree = &context.semantic.scope_tree;
        let mut collector = MutationCollector::default();
        collector.visit_block(context.block);
        tree.symbols
            .iter()
            .filter(|symbol| {
                collector.uninitialized_locals.contains(symbol) && !symbol.name.starts_with('_')
            })
            .filter_map(|symbol| {
                let first = first_reference(symbol, tree)?;
                // The scope tree records a field write's prefix as a `Read`.
                let is_mutation = collector.field_write_prefix_starts.contains(&first.span.start)
                    && first.kind == ReferenceKind::Read;
                is_mutation.then(|| {
                    LintDiagnostic::new(
                        "mutating_uninitialized",
                        format!(
                            "field/index assignment on uninitialized `{}` (value is `nil`)",
                            symbol.name
                        ),
                        first.span,
                    )
                    .with_help("initialize the local before mutating its fields")
                })
            })
            .collect()
    }
}

/// Records the locals declared without a value, and where a bare name is the
/// prefix of a field or index write (`x.foo = ...`, `x[k] = ...`).
#[derive(Default)]
struct MutationCollector {
    uninitialized_locals: UninitializedLocals,
    field_write_prefix_starts: Vec<u32>,
}

impl MutationCollector {
    fn record_target(&mut self, var: &Var) {
        let prefix = match var {
            Var::Name(_) => return,
            Var::FieldAccess(field_access) => &field_access.prefix,
            Var::Index(index) => &index.prefix,
        };
        if let Expression::Var(Var::Name(token)) = prefix
            && token.identifier().is_some()
        {
            self.field_write_prefix_starts.push(token.span.start);
        }
    }
}

impl<'ast> Visitor<'ast> for MutationCollector {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        self.uninitialized_locals.record(stmt);
        match stmt {
            Statement::Assignment(assignment) => {
                for var in assignment.targets.iter() {
                    self.record_target(var);
                }
            }
            // Luau: `x.foo += 1` also mutates a field.
            Statement::CompoundAssignment(compound) => self.record_target(&compound.var),
            Statement::LocalAssignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
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
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&MutatingUninitialized, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_field_write_on_uninitialized() {
        let diags = run("local x\nx.foo = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn flags_index_write_on_uninitialized() {
        let diags = run("local x\nx[1] = 2");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_when_initialized() {
        let diags = run("local x = {}\nx.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_when_assigned_first() {
        let diags = run("local x\nx = {}\nx.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_read_before_field_write() {
        let diags = run("local x\nprint(x)\nx.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("local _x\n_x.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
