use core::mem;

use luck_ast::{
    BinaryOp, Block, Expression, FunctionBody, LocalAssignment, Statement, SuffixedExpression,
    Transform, Var, Visitor, expr_nesting_depth, is_prefix_expr,
};
use luck_token::{BinOp, CompactString, LuaVersion, MAX_ACTIVE_LOCALS, OperandSide};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::binding::{
    IMPLICIT_VARARG_TABLE_NAME, METHOD_RECEIVER_NAME, has_fixed_binding, has_implicit_arg,
    identifier_name,
};
use crate::expr::{
    MAX_ADDED_DEPTH, binds_looser_than_unary, inner_operand_side, literal_kind, parenthesize,
};

/// This pass works on names, not bindings, so every candidate must be
/// position-insensitive:
/// - the name must not be bound anywhere else in the file (by another local,
///   a parameter, a loop variable, a `local function`, a 5.5 `global`, or a
///   function's implicit `self` or 5.1 `arg`), or a use of that binding would
///   get the candidate's value;
/// - every reference must be a read in the declaration's scope, since a read
///   before or past it is a global's and a write would become one;
/// - the value must be one no error or identity depends on: an operator over
///   literals may raise (`1 + nil` keeps its error before later statements),
///   a table or closure moved into a loop mints a fresh object per iteration,
///   and `...` means something else across a function boundary.
///
/// One scan finds the names that qualify, and one walk decides each where
/// it's declared, with the values declared before it already in place, so a
/// chain of as many locals as one function holds inlines whole. A value moves
/// to its one read.
pub(crate) fn inline(block: Block, version: LuaVersion) -> Block {
    let candidates = find_inline_candidates(&block, version);
    if candidates.is_empty() {
        return block;
    }
    let mut inliner = Inliner {
        candidates,
        values: FxHashMap::default(),
        has_bare_operand: false,
        landed_locals: 0,
        depth_limit: version.max_nesting_depth(),
    };
    let rewritten = inliner.transform_block(block);
    debug_assert!(inliner.values.is_empty(), "every inlined value reaches its read");
    rewritten
}

/// Each name that qualifies and the depth of its one read.
fn find_inline_candidates(block: &Block, version: LuaVersion) -> FxHashMap<CompactString, usize> {
    let mut scanner = CandidateScanner {
        declared_names: FxHashSet::default(),
        shadowed_names: FxHashSet::default(),
        references: FxHashMap::default(),
        open_declarations: FxHashSet::default(),
        opened_in_scope: Vec::new(),
        depth: 0,
        version,
    };
    scanner.visit_block(block);
    scanner
        .references
        .into_iter()
        .filter(|(name, reference)| {
            reference.count == 1
                && scanner.declared_names.contains(name)
                && !scanner.shadowed_names.contains(name)
        })
        .map(|(name, reference)| (name, reference.depth))
        .collect()
}

/// How often a name is read in its declaration's scope, and how deep the
/// first of those reads sits, which for a name read once is where its value
/// lands.
struct Reference {
    count: usize,
    depth: usize,
}

struct CandidateScanner {
    // Every single-name local with one initializer, so a second declaration
    // disqualifies the name.
    declared_names: FxHashSet<CompactString>,
    shadowed_names: FxHashSet<CompactString>,
    references: FxHashMap<CompactString, Reference>,
    /// Declared names whose scope the walk is inside.
    open_declarations: FxHashSet<CompactString>,
    /// `open_declarations` in opening order, to close them per scope.
    opened_in_scope: Vec<CompactString>,
    /// Levels of nesting open where the walk is.
    depth: usize,
    version: LuaVersion,
}

impl CandidateScanner {
    fn open_declaration(&mut self, name: &str) {
        self.open_declarations.insert(name.into());
        self.opened_in_scope.push(name.into());
    }

    fn close_scope(&mut self, scope_start: usize) {
        for name in self.opened_in_scope.drain(scope_start..) {
            self.open_declarations.remove(&name);
        }
    }

    fn shadow(&mut self, name: &str) {
        self.shadowed_names.insert(name.into());
    }

    /// A candidate is removed, so a write to its name would land on a global.
    fn shadow_write_target(&mut self, target: &Var) {
        if let Var::Name(name) = target {
            self.shadow(identifier_name(name));
        }
    }
}

impl<'ast> Visitor<'ast> for CandidateScanner {
    const COUNTS_LEVELS: bool = true;

    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        true
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn visit_block(&mut self, block: &'ast Block) {
        let scope_start = self.opened_in_scope.len();
        self.walk_block(block);
        self.close_scope(scope_start);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                if has_fixed_binding(local, self.version) {
                    for attributed in local.names.iter() {
                        self.shadowed_names.insert(identifier_name(&attributed.name).into());
                    }
                    self.walk_stmt(stmt);
                    return;
                }
                if let Some(name) = single_declared_name(local) {
                    // The local's scope starts after its statement, so a
                    // read inside it is another binding's.
                    self.walk_stmt(stmt);
                    if self.declared_names.contains(name) {
                        self.shadowed_names.insert(name.into());
                    } else {
                        self.declared_names.insert(name.into());
                        self.open_declaration(name);
                    }
                    return;
                }
                // Bare and multi-name locals are binders too: a reference
                // between `local x` and a later `local x = 1` resolves to this
                // one, so the name can't inline.
                for attributed in local.names.iter() {
                    self.shadowed_names.insert(identifier_name(&attributed.name).into());
                }
            }
            Statement::LocalFunction(local_function) => {
                self.shadowed_names.insert(identifier_name(&local_function.name).into());
            }
            Statement::NumericFor(numeric_for) => {
                self.shadowed_names.insert(identifier_name(&numeric_for.name).into());
            }
            Statement::GenericFor(generic_for) => {
                for param in generic_for.names.iter() {
                    self.shadowed_names.insert(identifier_name(&param.name).into());
                }
            }
            Statement::RepeatLoop(repeat_loop) => {
                // `until` reads the body's locals, so its scope closes after it.
                let scope_start = self.opened_in_scope.len();
                self.walk_block(&repeat_loop.block);
                self.visit_expr(&repeat_loop.condition);
                self.close_scope(scope_start);
                return;
            }
            Statement::Assignment(assignment) => {
                for target in assignment.targets.iter() {
                    self.shadow_write_target(target);
                }
            }
            // Luau
            Statement::CompoundAssignment(compound) => self.shadow_write_target(&compound.var),
            // `function x()` writes `x`, and `function x.y()` indexes it.
            Statement::FunctionDecl(function_decl) => {
                if let Some(root) = function_decl.name.names.first() {
                    self.shadow(identifier_name(root));
                }
                if function_decl.name.method.is_some() {
                    self.shadow(METHOD_RECEIVER_NAME);
                }
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.shadow(identifier_name(&global_function.name));
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global) => {
                for attributed in global.names.iter() {
                    self.shadow(identifier_name(&attributed.name));
                }
            }
            // These bind no local. `walk_stmt` below still descends into
            // nested blocks.
            Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::IfStatement(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        for param in body.params.iter() {
            self.shadow(identifier_name(&param.name));
        }
        // Lua 5.5
        if let Some(vararg_name) = body.vararg.as_ref().and_then(|vararg| vararg.name.as_ref()) {
            self.shadow(identifier_name(vararg_name));
        }
        if has_implicit_arg(body, self.version) {
            self.shadow(IMPLICIT_VARARG_TABLE_NAME);
        }
        self.walk_function_body(body);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Name(token) = expr {
            let name = identifier_name(token);
            if !self.open_declarations.contains(name) {
                self.shadow(name);
            } else if let Some(reference) = self.references.get_mut(name) {
                reference.count = reference.count.saturating_add(1);
            } else {
                self.references.insert(name.into(), Reference { count: 1, depth: self.depth });
            }
        }
        self.walk_expr(expr);
    }
}

/// The one name of a `local` with one name and one initializer.
fn single_declared_name(local: &LocalAssignment) -> Option<&str> {
    let initializers = local.exprs.as_ref().map(|exprs| exprs.items.as_slice());
    if let ([attributed], Some([_])) = (local.names.items.as_slice(), initializers) {
        Some(identifier_name(&attributed.name))
    } else {
        None
    }
}

/// A candidate's initializer, inlinable and waiting for the candidate's read.
struct Value {
    expr: Expression,
    /// The candidates it took in, its own included. Each is still in scope
    /// where its local is declared, so a chain past [`MAX_ACTIVE_LOCALS`] of
    /// them is longer than one function can hold, and it stops there.
    locals: usize,
}

struct Inliner {
    /// Each candidate's name and the depth of its one read.
    candidates: FxHashMap<CompactString, usize>,
    values: FxHashMap<CompactString, Value>,
    /// Whether a value has landed bare as an operand since the run being
    /// rebuilt began, so the run may hold one to splice.
    has_bare_operand: bool,
    /// The candidates taken in by the values landed since the initializer
    /// being rebuilt began.
    landed_locals: usize,
    depth_limit: usize,
}

impl Transform for Inliner {
    fn transform_block(&mut self, mut block: Block) -> Block {
        let stmts = mem::take(&mut block.stmts);
        block.stmts.reserve(stmts.len());
        for stmt in stmts {
            if let Statement::LocalAssignment(local) = stmt {
                block.stmts.extend(self.transform_local(local));
            } else {
                block.stmts.push(self.transform_stmt(stmt));
            }
        }
        if let Some(last) = &mut block.last_stmt {
            **last = self.transform_last_stmt(last.take());
        }
        block
    }

    /// A value inlined as the prefix gets parentheses unless it can stand
    /// bare there, as `x["k"]` with `x = 37.5` becomes `(37.5)["k"]`.
    fn transform_suffixed(&mut self, chain: Box<SuffixedExpression>) -> Box<SuffixedExpression> {
        let mut walked = self.walk_suffixed(chain);
        if !is_prefix_expr(&walked.prefix) {
            walked.prefix = parenthesize(walked.prefix.take());
        }
        walked
    }

    fn transform_expr(&mut self, expr: Expression) -> Expression {
        match expr {
            Expression::Name(name) => {
                if let Some(value) = self.values.remove(identifier_name(&name)) {
                    return self.land(value);
                }
                Expression::Name(name)
            }
            Expression::UnaryOp(mut unary_op) => {
                unary_op.operand = self.inline_operand(unary_op.operand, OperandPosition::Unary);
                Expression::UnaryOp(unary_op)
            }
            Expression::BinaryOp(run) => self.inline_run(run),
            // Luau
            Expression::TypeCast(mut cast) => {
                cast.expr = self.inline_operand(cast.expr, OperandPosition::Cast);
                cast.type_annotation = self.transform_type(cast.type_annotation);
                Expression::TypeCast(cast)
            }
            other @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::Error(_)) => self.walk_expr(other),
        }
    }
}

#[derive(Clone, Copy)]
enum OperandPosition {
    Left(BinOp),
    Right(BinOp),
    Unary,
    Cast,
}

/// Grouping lives in `Parenthesized` nodes and codegen prints the tree flat,
/// so a value that binds looser than its position would regroup on reparse.
///
/// A value is a closed literal expression, so none prints a Luau
/// if-expression last.
fn keeps_grouping(value: &Expression, position: OperandPosition) -> bool {
    match value {
        Expression::BinaryOp(run) => match position {
            OperandPosition::Left(op) => run.can_nest_bare(op, OperandSide::Left),
            OperandPosition::Right(op) => run.can_nest_bare(op, OperandSide::Right),
            OperandPosition::Unary => !binds_looser_than_unary(run),
            OperandPosition::Cast => false,
        },
        Expression::UnaryOp(_) => match position {
            // Unary ops bind looser than ^, so (-a)^b needs its parens.
            OperandPosition::Left(op) => op != BinOp::Pow,
            OperandPosition::Right(_) | OperandPosition::Unary => true,
            OperandPosition::Cast => false,
        },
        // Luau: `::` takes a simple expression, and `x :: A :: B` doesn't
        // parse.
        Expression::TypeCast(_) => !matches!(position, OperandPosition::Cast),
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => true,
    }
}

/// The operand `expr` prints last, which is what a following token lands on.
fn last_operand(expr: &mut Expression) -> &mut Expression {
    let mut current = expr;
    loop {
        current = match current {
            Expression::BinaryOp(binary_op) => &mut binary_op.links.last_mut().operand,
            Expression::UnaryOp(unop) => &mut unop.operand,
            Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::Nil(_)
            | Expression::True(_)
            | Expression::False(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => return current,
        };
    }
}

impl Inliner {
    /// Rewrites a `local`. A candidate's goes, and its value waits for the
    /// candidate's read, when the value is inlinable.
    fn transform_local(&mut self, mut local: Box<LocalAssignment>) -> Option<Statement> {
        let Some(read_depth) =
            single_declared_name(&local).and_then(|name| self.candidates.get(name).copied())
        else {
            return Some(self.walk_stmt(Statement::LocalAssignment(local)));
        };
        local.names = self.walk_attributed_names(local.names);
        if let (Some(exprs), [attributed]) = (&mut local.exprs, local.names.items.as_slice())
            && let [initializer] = exprs.items.as_mut_slice()
        {
            let outer_landed_locals = mem::replace(&mut self.landed_locals, 0);
            *initializer = self.transform_expr(initializer.take());
            let locals =
                mem::replace(&mut self.landed_locals, outer_landed_locals).saturating_add(1);
            if locals <= MAX_ACTIVE_LOCALS && literal_kind(initializer).is_some() {
                let depth = expr_nesting_depth(initializer);
                // The value can gain parentheses where it lands, and a cast
                // inside it a second pair.
                if depth <= MAX_ADDED_DEPTH
                    && read_depth.saturating_add(2).saturating_add(depth) <= self.depth_limit
                {
                    let value = Value { expr: initializer.take(), locals };
                    self.values.insert(identifier_name(&attributed.name).into(), value);
                    return None;
                }
            }
        }
        Some(Statement::LocalAssignment(local))
    }

    fn land(&mut self, value: Value) -> Expression {
        self.landed_locals = self.landed_locals.saturating_add(value.locals);
        value.expr
    }

    /// Only in an operand position can a value regroup, so only there does
    /// it gain parens. Anywhere else it stays bare, since `return (1+2)` is
    /// longer than `return 1+2` and `simplify_parens` wouldn't strip those
    /// parens.
    fn inline_operand(&mut self, operand: Expression, position: OperandPosition) -> Expression {
        if let Expression::Name(name) = &operand
            && let Some(value) = self.values.remove(identifier_name(name))
        {
            let expr = self.land(value);
            if keeps_grouping(&expr, position) {
                self.has_bare_operand = true;
                return expr;
            }
            return parenthesize(expr);
        }
        self.transform_expr(operand)
    }

    /// A value inlined bare where it would merge into its operator's run
    /// joins it, so a run of locals inlines without nesting.
    fn inline_run(&mut self, mut run: Box<BinaryOp>) -> Expression {
        // The operand before each `<` is checked while it's still a name.
        let mut before = &mut run.first;
        for link in &mut run.links {
            if link.op == BinOp::Lt {
                self.close_cast_before_less_than(before);
            }
            before = &mut link.operand;
        }
        let op = run.links.first().op;
        let inner_position = match inner_operand_side(&run) {
            OperandSide::Left => OperandPosition::Left(op),
            OperandSide::Right => OperandPosition::Right(op),
        };
        let outer_has_bare_operand = mem::replace(&mut self.has_bare_operand, false);
        run.first = self.inline_operand(run.first.take(), OperandPosition::Left(op));
        let last_index = run.links.len().saturating_sub(1);
        for (i, link) in run.links.iter_mut().enumerate() {
            let position =
                if i == last_index { OperandPosition::Right(op) } else { inner_position };
            link.operand = self.inline_operand(link.operand.take(), position);
        }
        if mem::replace(&mut self.has_bare_operand, outer_has_bare_operand) {
            run.splice_operands();
        }
        Expression::BinaryOp(run)
    }

    /// Luau: a cast has no closing token and `T<` opens generic arguments, so
    /// a cast inlined just before `<` needs parens of its own. A cast that was
    /// already there parsed fine, so only inlining can cause this.
    fn close_cast_before_less_than(&mut self, left: &mut Expression) {
        if let Expression::Name(name) = last_operand(left)
            && let Some(value) = self.values.get_mut(identifier_name(name))
        {
            let cast = last_operand(&mut value.expr);
            if matches!(cast, Expression::TypeCast(_)) {
                *cast = parenthesize(cast.take());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use luck_ast::LastStatement;

    use super::*;
    use crate::fixtures::{assert_reparses, parse, run_pass};

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    fn apply_version(source: &str, version: LuaVersion) -> String {
        run_pass(source, version, |block| inline(block, version))
    }

    #[test]
    fn inlines_single_use_number() {
        let result = apply(
            "\
local x = 42
return x
",
        );
        assert!(!result.contains("local"), "Declaration not removed: {result}");
        assert!(result.contains("42"), "Value not inlined: {result}");
    }

    #[test]
    fn no_inline_of_multi_use_local() {
        let result = apply(
            "\
local x = 42
print(x)
return x
",
        );
        assert!(result.contains("local"), "Multi-use local was inlined: {result}");
    }

    #[test]
    fn no_inline_of_call_initializer() {
        let result = apply(
            "\
local x = foo()
return x
",
        );
        assert!(result.contains("local"), "Side-effectful init was inlined: {result}");
    }

    #[test]
    fn no_inline_of_variable_read() {
        let result = apply(
            "\
local x = y
return x
",
        );
        assert!(result.contains("local"), "Variable read was inlined: {result}");
    }

    #[test]
    fn inlines_in_nested_function_body() {
        let result = apply(
            "\
local function foo()
  local x = 42
  return x
end
return foo
",
        );
        assert!(
            !result.contains("local x"),
            "Declaration inside function body not inlined: {result}"
        );
    }

    #[test]
    fn inlines_in_if_block() {
        let result = apply(
            "\
if true then
  local x = 42
  print(x)
end
",
        );
        assert!(!result.contains("local x"), "Declaration inside if block not inlined: {result}");
    }

    #[test]
    fn parenthesizes_literal_inlined_into_index_prefix() {
        let result = apply(
            "\
local x = 37.5
return x[\"k\"]
",
        );
        assert!(
            result.contains("(37.5)[\"k\"]"),
            "Literal in index-prefix position must gain parens: {result}"
        );
    }

    #[test]
    fn parenthesizes_literal_inlined_into_field_prefix() {
        let result = apply(
            "\
local x = \"s\"
return x.len
",
        );
        assert!(
            result.contains("(\"s\").len"),
            "Literal in field-prefix position must gain parens: {result}"
        );
    }

    fn apply_luau_reparsing(source: &str) -> String {
        let output = apply_version(source, LuaVersion::Luau);
        assert_reparses(&output, LuaVersion::Luau);
        output
    }

    #[test]
    fn parenthesizes_literal_inlined_into_type_instantiation() {
        for (source, expected) in [
            (
                "\
local s = \"x\"
return s<<T>>
",
                "return(\"x\")<<T>>",
            ),
            (
                "\
local s = 1
return s<<T>>
",
                "return(1)<<T>>",
            ),
            (
                "\
local s = nil
return s<<T>>
",
                "return(nil)<<T>>",
            ),
            (
                "\
local s = -1
return s<<T>>
",
                "return(-1)<<T>>",
            ),
            (
                "\
local s = \"x\" :: T
return s<<U>>
",
                "return(\"x\"::T)<<U>>",
            ),
            (
                "\
local s = \"x\"
return s<<T>><<U>>
",
                "return(\"x\")<<T>><<U>>",
            ),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn literal_inlined_under_instantiation_reparses_as_prefix() {
        apply_luau_reparsing(
            "\
local s = \"x\"
return s<<T>>()
",
        );
        apply_luau_reparsing(
            "\
local s = \"x\"
return s<<T>>.y
",
        );
        apply_luau_reparsing(
            "\
local s = \"x\"
s<<T>>()
",
        );
    }

    #[test]
    fn leaves_chained_type_instantiation_unwrapped() {
        // A chain is already a valid prefix.
        let source = "\
local s = 1
print(s)
return f<<T>><<U>>
";
        assert_eq!(apply_luau_reparsing(source), "print(1)return f<<T>><<U>>");
    }

    #[test]
    fn leaves_type_instantiation_prefixes_unwrapped() {
        for (source, expected) in [
            (
                "\
local s = 1
print(s)
return f<<T>>(x)
",
                "print(1)return f<<T>>(x)",
            ),
            (
                "\
local s = 1
print(s)
return f<<T>>.y
",
                "print(1)return f<<T>>.y",
            ),
            (
                "\
local s = 1
print(s)
f<<T>>(x)
",
                "print(1)f<<T>>(x)",
            ),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    fn assert_inlines(cases: &[(&str, &str)]) {
        for &(source, expected) in cases {
            let result = apply(source);
            assert_reparses(&result, LuaVersion::Lua54);
            assert_eq!(result, expected, "source: {source}");
        }
    }

    #[test]
    fn parenthesizes_inlined_operator_that_would_regroup() {
        assert_inlines(&[
            (
                "\
local s = 1 / 0
return 3 % s
",
                "return 3%(1/0)",
            ),
            (
                "\
local s = 1 + 2
return s * 2
",
                "return(1+2)*2",
            ),
            (
                "\
local s = 1 + 2
return 2 * s
",
                "return 2*(1+2)",
            ),
            (
                "\
local s = 1 - 2
return 5 - s
",
                "return 5-(1-2)",
            ),
            (
                "\
local s = 2 ^ 3
return s ^ 2
",
                "return(2^3)^2",
            ),
            (
                "\
local s = 1 .. 2
return s .. 3
",
                "return(1 ..2)..3",
            ),
            (
                "\
local s = 1 + 2
return 2 ^ s
",
                "return 2^(1+2)",
            ),
            (
                "\
local s = 1 | 2
return s & 3
",
                "return(1|2)&3",
            ),
            (
                "\
local s = 1 or 2
return s and 3
",
                "return(1 or 2)and 3",
            ),
            (
                "\
local s = -2
return s ^ y
",
                "return(-2)^y",
            ),
            (
                "\
local s = not nil
return s ^ y
",
                "return(not nil)^y",
            ),
        ]);
    }

    #[test]
    fn parenthesizes_operator_inlined_under_a_unary() {
        assert_inlines(&[
            (
                "\
local s = 1 + 2
return -s
",
                "return-(1+2)",
            ),
            (
                "\
local s = 1 == 2
return not s
",
                "return not(1==2)",
            ),
            (
                "\
local s = \"a\" .. \"b\"
return #s
",
                "return#(\"a\"..\"b\")",
            ),
            (
                "\
local s = 1 * 2
return ~s
",
                "return~(1*2)",
            ),
        ]);
    }

    #[test]
    fn parenthesizes_operator_inlined_into_a_nested_operand() {
        assert_inlines(&[
            (
                "\
local s = 1 + 2
return -s ^ 2
",
                "return-(1+2)^2",
            ),
            (
                "\
local s = -1
return -s ^ 2
",
                "return-(-1)^2",
            ),
            (
                "\
local s = 1 + 2
return (s * 3)
",
                "return((1+2)*3)",
            ),
            (
                "\
local s = 1 + 2
print(s * 3, 4)
",
                "print((1+2)*3,4)",
            ),
            (
                "\
local s = 1 + 2
return {s * 3}
",
                "return{(1+2)*3}",
            ),
            (
                "\
local s = 1 + 2
return t[s * 3]
",
                "return t[(1+2)*3]",
            ),
            (
                "\
local s = 1 + 2
if s * 3 then end
",
                "if(1+2)*3 then end",
            ),
            (
                "\
local a = 1 + 2
local b = a * 3
return b ^ 2
",
                "return((1+2)*3)^2",
            ),
        ]);
    }

    const BARE_INLINE_CASES: &[(&str, &str)] = &[
        (
            "\
local s = 1 + 2
return s
",
            "return 1+2",
        ),
        (
            "\
local s = 1 + 2
print(s)
",
            "print(1+2)",
        ),
        (
            "\
local s = 1 + 2
return {s}
",
            "return{1+2}",
        ),
        (
            "\
local s = 1 + 2
return t[s]
",
            "return t[1+2]",
        ),
        (
            "\
local s = 1 * 2
return s + 3
",
            "return 1*2+3",
        ),
        (
            "\
local s = 1 * 2
return 3 + s
",
            "return 3+1*2",
        ),
        (
            "\
local s = 1 - 2
return s - 5
",
            "return 1-2-5",
        ),
        (
            "\
local s = 2 ^ 3
return 2 ^ s
",
            "return 2^2^3",
        ),
        (
            "\
local s = 1 .. 2
return 3 .. s
",
            "return 3 ..1 ..2",
        ),
        (
            "\
local s = 1 + 2
return s < 4
",
            "return 1+2<4",
        ),
        (
            "\
local s = 1 < 2
return s == true
",
            "return 1<2==true",
        ),
        (
            "\
local s = 2 ^ 3
return -s
",
            "return-2^3",
        ),
        (
            "\
local s = -2
return 3 ^ s
",
            "return 3^-2",
        ),
        (
            "\
local s = -2
return s * 3
",
            "return-2*3",
        ),
        (
            "\
local s = -1
return -s
",
            "return- -1",
        ),
        (
            "\
local s = (1 + 2)
return s * 2
",
            "return(1+2)*2",
        ),
        (
            "\
local s = 2
return s * 3
",
            "return 2*3",
        ),
    ];

    #[test]
    fn leaves_inlined_expr_bare_where_its_grouping_holds() {
        assert_inlines(BARE_INLINE_CASES);
    }

    #[test]
    fn parenthesizes_expr_inlined_under_a_cast() {
        // `::` binds to a simple expression and doesn't chain.
        for (source, expected) in [
            (
                "\
local s = 1 + 2
return s :: any
",
                "return(1+2)::any",
            ),
            (
                "\
local s = -1
return s :: any
",
                "return(-1)::any",
            ),
            (
                "\
local s = 1 :: number
return s :: any
",
                "return(1::number)::any",
            ),
            (
                "\
local s = 1
return s :: any
",
                "return 1::any",
            ),
            (
                "\
local s = (1 + 2)
return s :: any
",
                "return(1+2)::any",
            ),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn parenthesizes_cast_inlined_before_less_than() {
        // `T<` opens generic arguments, so the cast has to end before the `<`.
        for (source, expected) in [
            (
                "\
local s = 1 :: number
return s < 2
",
                "return(1::number)<2",
            ),
            (
                "\
local s = 1 :: number
return 2 + s < 3
",
                "return 2+(1::number)<3",
            ),
            (
                "\
local s = 1 :: number
return -s < 2
",
                "return-(1::number)<2",
            ),
            (
                "\
local s = 1 :: number
return 1 < s < 3
",
                "return 1<(1::number)<3",
            ),
            (
                "\
local s = 2 + 1 :: number
return s < 3
",
                "return 2+(1::number)<3",
            ),
            (
                "\
local s = -1 :: number
return s < 3
",
                "return-(1::number)<3",
            ),
            (
                "\
local s = 2 + 1 :: number
return 2 * s < 3
",
                "return 2*(2+(1::number))<3",
            ),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn leaves_cast_bare_where_nothing_extends_its_type() {
        for (source, expected) in [
            (
                "\
local s = 1 :: number
return s
",
                "return 1::number",
            ),
            (
                "\
local s = 1 :: number
return s + 1
",
                "return 1::number+1",
            ),
            (
                "\
local s = 1 :: number
return 1 + s
",
                "return 1+1::number",
            ),
            (
                "\
local s = 1 :: number
return -s
",
                "return-1::number",
            ),
            (
                "\
local s = 1 :: number
return s > 2
",
                "return 1::number>2",
            ),
            (
                "\
local s = 1 :: number
return 2 < s
",
                "return 2<1::number",
            ),
            (
                "\
local s = 1
return x :: Foo<T> < s
",
                "return x::Foo<T><1",
            ),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn no_inline_of_an_instantiated_literal() {
        // Only a function takes type arguments.
        let result = apply_luau_reparsing(
            "\
local s = (\"x\")<<T>>
return s .. \"y\"
",
        );
        assert_eq!(result, "local s=(\"x\")<<T>>return s..\"y\"");
    }

    #[test]
    fn parenthesizes_literal_inlined_into_stmt_call_receiver() {
        let result = apply(
            "\
local x = false
x:resize(9)
",
        );
        assert!(
            result.contains("(false):resize(9)"),
            "Literal receiver of a statement call must gain parens: {result}"
        );
    }

    #[test]
    fn no_inline_of_table_constructor() {
        // Tables carry identity: moved into a loop, `{1,2,3}` would mint
        // a fresh table per iteration.
        let result = apply(
            "\
local t = {1, 2, 3}
return t
",
        );
        assert!(result.contains("local"), "Identity value was inlined: {result}");
    }

    #[test]
    fn no_inline_of_function_def() {
        // Closures carry identity and captures, and both depend on position:
        // in `local a=1 local f=function() return a end local a=2`, moving f's
        // body past the second `a` would rebind it.
        let result = apply(
            "\
local f = function() return 1 end
return f
",
        );
        assert!(result.contains("local"), "Closure was inlined: {result}");
    }

    #[test]
    fn inlines_chained_single_use() {
        // `x` is decided first, so `y`'s initializer is `42 + 1` by the time
        // `y` is, and both inline in the one walk.
        let result = apply(
            "\
local x = 42
local y = x + 1
return y
",
        );
        assert_eq!(result, "return 42+1");
    }

    /// `local x0 = 1`, then `local xi = x(i-1) + 1` up to `x(links)`, returned.
    fn sum_chain(links: usize) -> String {
        let mut source = String::from("local x0 = 1\n");
        for (i, previous) in (1..=links).zip(0usize..) {
            writeln!(source, "local x{i} = x{previous} + 1").unwrap();
        }
        writeln!(source, "return x{links}").unwrap();
        source
    }

    #[test]
    fn inlines_a_chain_of_as_many_locals_as_one_function_holds() {
        let links = MAX_ACTIVE_LOCALS - 1;
        let block = inline(parse(&sum_chain(links), LuaVersion::Lua54), LuaVersion::Lua54);
        let Some(LastStatement::Return(return_stmt)) = block.last_stmt.as_deref() else {
            panic!("expected a return, got {block:?}");
        };
        let run = return_stmt.exprs.items.first().expect("the return has its value");
        assert_eq!(expr_nesting_depth(run), 1, "the chain splices into one run");
        assert_eq!(luck_codegen::compact(&block), format!("return 1{}", "+1".repeat(links)));

        let mut concat = String::from("local x0 = \"z\"\n");
        for (i, previous) in (1..=links).zip(0usize..) {
            writeln!(concat, "local x{i} = \"a\" .. x{previous}").unwrap();
        }
        writeln!(concat, "return x{links}").unwrap();
        assert_eq!(apply(&concat), format!("return{}\"z\"", "\"a\"..".repeat(links)));
    }

    #[test]
    fn stops_a_chain_past_the_locals_one_function_holds() {
        let links = MAX_ACTIVE_LOCALS;
        let expected = format!("local x{links}=1{} return x{links}", "+1".repeat(links));
        assert_eq!(apply(&sum_chain(links)), expected);
    }

    #[test]
    fn stops_inlining_a_run_of_locals_before_it_nests_past_the_depth_cap() {
        // Each value needs parentheses where the next one uses it.
        let links = MAX_ACTIVE_LOCALS - 1;
        let mut source = String::from("local x0 = 1\n");
        for (i, previous) in (1..=links).zip(0usize..) {
            writeln!(source, "local x{i} = x{previous} * 2 + 1").unwrap();
        }
        writeln!(source, "return x{links}").unwrap();
        let result = apply(&source);
        assert_reparses(&result, LuaVersion::Lua54);
        assert!(result.contains("local"), "the run must stop inlining partway: {result}");
    }

    #[test]
    fn a_run_inlined_bare_joins_the_run_it_lands_in() {
        // `1 + 2` lands on the left of `a - x`, and `3 .. 4` on the right of
        // `y .. b`.
        let source = "\
local a = 1 + 2
local b = 3 .. 4
return a - x, y .. b
";
        let block = inline(parse(source, LuaVersion::Lua54), LuaVersion::Lua54);
        let Some(LastStatement::Return(return_stmt)) = block.last_stmt.as_deref() else {
            panic!("expected a return, got {block:?}");
        };
        assert_eq!(return_stmt.exprs.len(), 2);
        for run in return_stmt.exprs.iter() {
            assert_eq!(expr_nesting_depth(run), 1, "{run:?}");
        }
    }

    #[test]
    fn no_inline_of_vararg() {
        // Inside a nested function, `...` is that function's own varargs, or
        // a compile error if it takes none.
        let result = apply(
            "\
local function f(...)
  local x = ...
  return x
end
",
        );
        assert!(result.contains("local x"), "VarArg was inlined: {result}");
    }

    #[test]
    fn no_inline_of_multi_use_vararg() {
        let result = apply(
            "\
local function f(...)
  local x = ...
  print(x)
  return x
end
",
        );
        assert!(result.contains("local x"), "Multi-use vararg should not be inlined: {result}");
    }

    #[test]
    fn no_inline_of_variable_read_under_cast() {
        // TypeCast is transparent, and the inner variable read is impure.
        let result = apply_version(
            "\
local x = foo :: Bar
return x
",
            LuaVersion::Luau,
        );
        assert!(result.contains("local"), "TypeCast with var read should not be inlined: {result}");
    }

    #[test]
    fn no_inline_over_a_read_outside_the_declaration_scope() {
        for source in [
            "\
print(x)
local x = 1
",
            "\
local function g() return x end
local x = 1
print(g())
",
            "\
do local x = 1 end
print(x)
",
            "\
for i = 1, 1 do
  print(x)
  local x = 2
end
",
        ] {
            let result = apply(source);
            assert!(
                result.contains("print(x)") || result.contains("return x"),
                "{source}: {result}"
            );
        }
    }

    #[test]
    fn inlines_reads_inside_the_declaration_scope() {
        assert_inlines(&[
            (
                "\
local x = 1
print(x)
",
                "print(1)",
            ),
            (
                "\
local x = 1
local function f() return x end
",
                "local function f()return 1 end",
            ),
            (
                "\
if c then
  local x = 1
  print(x)
end
",
                "if c then print(1)end",
            ),
            (
                "\
do
  local x = 1
  do print(x) end
end
",
                "do do print(1)end end",
            ),
            (
                "\
repeat
  local x = 1
until x == y
",
                "repeat until 1==y",
            ),
            (
                "\
repeat
  local x = 1
  repeat until x
until z
",
                "repeat repeat until 1 until z",
            ),
        ]);
    }

    #[test]
    fn no_inline_over_a_write_to_the_name() {
        for source in [
            "\
local x = 1
x = 2
",
            "\
local x = 1
function x() end
print(x)
",
            "\
local x = \"s\"
function x.f() end
print(x)
",
        ] {
            let result = apply(source);
            assert!(result.contains("local x"), "{source}: {result}");
        }
        let compound = apply_version(
            "\
local x = 1
x += 2
",
            LuaVersion::Luau,
        );
        assert!(compound.contains("local x"), "{compound}");
    }

    #[test]
    fn no_inline_over_lua55_bindings() {
        for source in [
            "\
local args = 1
local function f(...args) return args end
",
            "\
local x = 1
global x
print(x)
",
            "\
local x = 1
global function x() end
print(x)
",
        ] {
            let result = apply_version(source, LuaVersion::Lua55);
            assert!(
                result.contains("local args") || result.contains("local x"),
                "{source}: {result}"
            );
        }
    }

    #[test]
    fn no_inline_across_global_function_shadow() {
        // The outer `x` is never read, and the `global function` body
        // declares its own `x` and reads it once. The scan has to see that
        // second declaration, or the outer value leaks in as
        // `global function g()return 1 end`.
        let result = apply_version(
            "\
local x = 1
global function g()
  local x = 2
  return x
end
return g
",
            LuaVersion::Lua55,
        );
        assert!(
            result.contains("local x=2")
                || result.contains("local x =2")
                || result.contains("return 2"),
            "Inner shadowed local must be preserved, not replaced by outer value: {result}"
        );
        assert!(
            !result.contains("return 1"),
            "Outer value must not leak into inner scope: {result}"
        );
    }

    #[test]
    fn no_inline_into_a_method_that_binds_self() {
        let result = apply(
            "\
local self = 5
function M:get() return self end
",
        );
        assert!(
            result.contains("return self"),
            "`self` in a `:` method is the receiver, not the outer local: {result}"
        );
    }

    #[test]
    fn no_inline_into_a_lua51_vararg_function_that_binds_arg() {
        let result = apply_version(
            "\
local arg = 5
local function f(...) return arg end
return f
",
            LuaVersion::Lua51,
        );
        assert!(
            result.contains("return arg"),
            "`arg` in a 5.1 vararg function is its implicit table, not the outer local: {result}"
        );
    }

    #[test]
    fn no_inline_of_a_literal_operation_that_raises() {
        let result = apply(
            "\
local x = 1 + nil
print(\"before\")
return x
",
        );
        assert!(
            !result.starts_with("print"),
            "the error must still fire before the print runs: {result}"
        );
    }
}
