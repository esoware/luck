use core::mem;

use luck_ast::{Block, Expression, FunctionBody, Statement, Transform, Var, Visitor};
use luck_token::{BinOp, CompactString, LuaVersion, Span, TokenKind, UnOp};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::binding::{has_fixed_binding, identifier_name};
use crate::expr::{can_unwrap_in_binop_lhs, can_unwrap_in_binop_rhs, parenthesize};

/// Inlines single-use locals whose initializer is a closed literal
/// expression, removing the declaration.
///
/// This pass works on names, not bindings, so every candidate must be
/// position-insensitive:
/// - the name must not be bound anywhere else in the file (by another local,
///   a parameter, a loop variable, or a `local function`), or a use of that
///   binding would get the candidate's value;
/// - the value must carry no identity (a table or closure moved into a loop
///   mints a fresh object per iteration), capture nothing, and not be `...`,
///   whose meaning changes across function boundaries.
pub(crate) fn inline(mut block: Block, version: LuaVersion) -> Block {
    loop {
        let candidates = find_inline_candidates(&block, version);
        if candidates.is_empty() {
            break;
        }
        let mut inliner = Inliner { candidates };
        block = inliner.transform_block(block);
    }
    block
}

/// Whether `expr` is a closed literal: literals and operators over literals,
/// with nothing whose value or identity depends on where it's evaluated.
fn is_closed_literal_expr(expr: &Expression) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_) => true,
        Expression::Parenthesized(paren) => is_closed_literal_expr(&paren.expr),
        Expression::UnaryOp(unop) => is_closed_literal_expr(&unop.operand),
        Expression::BinaryOp(binop) => {
            is_closed_literal_expr(&binop.left) && is_closed_literal_expr(&binop.right)
        }
        // Luau
        Expression::TypeCast(cast) => is_closed_literal_expr(&cast.expr),
        // Luau
        Expression::TypeInstantiation(instantiation) => is_closed_literal_expr(&instantiation.expr),
        Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => false,
    }
}

/// Each inlinable local's name, mapped to its initializer.
fn find_inline_candidates(
    block: &Block,
    version: LuaVersion,
) -> FxHashMap<CompactString, Expression> {
    let mut scanner = CandidateScanner {
        declarations: FxHashMap::default(),
        declared_names: FxHashSet::default(),
        shadowed_names: FxHashSet::default(),
        reference_counts: FxHashMap::default(),
        version,
    };
    scanner.visit_block(block);

    scanner
        .declarations
        .into_iter()
        .filter(|(name, _)| {
            !scanner.shadowed_names.contains(name) && scanner.reference_counts.get(name) == Some(&1)
        })
        .collect()
}

/// Gathers literal declarations, disqualifying binders, and per-name reference
/// counts for `find_inline_candidates` in one walk.
struct CandidateScanner {
    declarations: FxHashMap<CompactString, Expression>,
    // Every single-name local with one initializer, literal or not, so a
    // second declaration disqualifies the name even if only one of the two
    // initializers is a literal.
    declared_names: FxHashSet<CompactString>,
    shadowed_names: FxHashSet<CompactString>,
    reference_counts: FxHashMap<CompactString, usize>,
    version: LuaVersion,
}

impl<'ast> Visitor<'ast> for CandidateScanner {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                if has_fixed_binding(local, self.version) {
                    for attributed in local.names.iter() {
                        self.shadowed_names.insert(identifier_name(&attributed.name).into());
                    }
                    self.walk_statement(stmt);
                    return;
                }
                let initializers = local.exprs.as_ref().map(|exprs| exprs.items.as_slice());
                if let ([attributed], Some([expr])) = (local.names.items.as_slice(), initializers) {
                    let name = identifier_name(&attributed.name);
                    if self.declared_names.contains(name) {
                        self.shadowed_names.insert(name.into());
                    } else {
                        self.declared_names.insert(name.into());
                        if is_closed_literal_expr(expr) {
                            self.declarations.insert(name.into(), expr.clone());
                        }
                    }
                } else {
                    // Bare and multi-name locals are binders too: a
                    // reference between `local x` and a later `local x = 1`
                    // resolves to this one, so the name can't inline.
                    for attributed in local.names.iter() {
                        self.shadowed_names.insert(identifier_name(&attributed.name).into());
                    }
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
            // These bind no local. `walk_statement` below still descends into
            // nested blocks.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::FunctionDecl(_)
            | Statement::GlobalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
        self.walk_statement(stmt);
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        for param in body.params.iter() {
            if let TokenKind::Identifier(name) = &param.name.kind {
                self.shadowed_names.insert(name.clone());
            }
        }
        self.walk_function_body(body);
    }

    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(name) = var {
            if let Some(count) = self.reference_counts.get_mut(identifier_name(name)) {
                *count = count.saturating_add(1);
            } else {
                self.reference_counts.insert(identifier_name(name).into(), 1);
            }
        }
        self.walk_var(var);
    }
}

struct Inliner {
    candidates: FxHashMap<CompactString, Expression>,
}

impl Transform for Inliner {
    fn transform_block(&mut self, block: Block) -> Block {
        let stmts: Vec<_> = block
            .stmts
            .into_iter()
            .filter_map(|stmt| {
                let transformed = self.transform_statement(stmt);
                (!self.is_inlined_declaration(&transformed)).then_some(transformed)
            })
            .collect();

        let last_stmt = block.last_stmt.map(|mut last| {
            *last = self.transform_last_statement(*last);
            last
        });

        Block { span: block.span, stmts, last_stmt }
    }

    fn transform_statement(&mut self, stmt: Statement) -> Statement {
        let walked = self.walk_statement(stmt);
        // A call statement holds its `FunctionCall` outside any `Expression`,
        // so `transform_expression` never guards its callee.
        if let Statement::FunctionCall(mut call_stmt) = walked {
            call_stmt.callee = ensure_prefix(call_stmt.callee);
            return Statement::FunctionCall(call_stmt);
        }
        walked
    }

    fn transform_var(&mut self, var: Var) -> Var {
        match self.walk_var(var) {
            Var::Index(mut index_expr) => {
                index_expr.prefix = ensure_prefix(index_expr.prefix);
                Var::Index(index_expr)
            }
            Var::FieldAccess(mut field_access) => {
                field_access.prefix = ensure_prefix(field_access.prefix);
                Var::FieldAccess(field_access)
            }
            name @ Var::Name(_) => name,
        }
    }

    fn transform_expression(&mut self, mut expr: Expression) -> Expression {
        self.inline_operands(&mut expr);
        let walked = self.walk_expression(expr);
        if let Expression::FunctionCall(mut call) = walked {
            call.callee = ensure_prefix(call.callee);
            return Expression::FunctionCall(call);
        }
        // Luau: a chained `f<<T>><<U>>` is already a valid prefix.
        if let Expression::TypeInstantiation(mut instantiation) = walked {
            if !matches!(instantiation.expr, Expression::TypeInstantiation(_)) {
                instantiation.expr = ensure_prefix(instantiation.expr);
            }
            return Expression::TypeInstantiation(instantiation);
        }
        if let Expression::Var(Var::Name(name)) = &walked
            && let Some(initializer) = self.candidates.get(identifier_name(name))
        {
            return initializer.clone();
        }
        walked
    }
}

/// Parenthesizes an inlined literal that landed in a prefix position.
///
/// Prefix positions (index and field prefixes, callees, type instantiations)
/// only take vars, calls, and parenthesized expressions, so `x["k"]` with
/// `x = 37.5` has to become `(37.5)["k"]`.
fn ensure_prefix(expr: Expression) -> Expression {
    if matches!(
        expr,
        Expression::Var(_) | Expression::FunctionCall(_) | Expression::Parenthesized(_)
    ) {
        return expr;
    }
    parenthesize(expr)
}

#[derive(Clone, Copy)]
enum OperandPosition {
    Left(BinOp),
    Right(BinOp),
    Unary,
    Cast,
}

/// Whether `expr` printed bare in `position` reparses as that same operand.
///
/// Grouping lives in `Parenthesized` nodes and codegen prints the tree flat, so
/// an operand that binds looser than its position silently regroups on reparse.
fn keeps_grouping(expr: &Expression, position: OperandPosition) -> bool {
    match expr {
        Expression::BinaryOp(_) | Expression::UnaryOp(_) => match position {
            OperandPosition::Left(op) => can_unwrap_in_binop_lhs(expr, op),
            OperandPosition::Right(op) => can_unwrap_in_binop_rhs(expr, op),
            OperandPosition::Unary => !matches!(
                expr,
                Expression::BinaryOp(binop) if binop.op.precedence().0 < UnOp::PRECEDENCE
            ),
            OperandPosition::Cast => false,
        },
        // Luau: `::` takes a simple expression, and `x :: A :: B` doesn't
        // parse.
        Expression::TypeCast(_) => match position {
            OperandPosition::Left(_) | OperandPosition::Right(_) | OperandPosition::Unary => true,
            OperandPosition::Cast => false,
        },
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => true,
    }
}

/// The operand `expr` prints last, which is what a following token lands on.
fn last_operand(expr: &mut Expression) -> &mut Expression {
    match expr {
        Expression::BinaryOp(binop) => last_operand(&mut binop.right),
        Expression::UnaryOp(unop) => last_operand(&mut unop.operand),
        operand @ (Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_)) => operand,
    }
}

impl Inliner {
    fn is_inlined_declaration(&self, stmt: &Statement) -> bool {
        if let Statement::LocalAssignment(local) = stmt
            && let [attributed] = local.names.items.as_slice()
        {
            return self.candidates.contains_key(identifier_name(&attributed.name));
        }
        false
    }

    /// Inlines the candidates `expr` reads directly as operands.
    ///
    /// Only in an operand position can a replacement regroup, so only there does it
    /// gain parens. Anywhere else it stays bare, since `return (1+2)` is longer
    /// than `return 1+2` and `simplify_parens` wouldn't strip those parens.
    fn inline_operands(&mut self, expr: &mut Expression) {
        match expr {
            Expression::BinaryOp(binop) => {
                if binop.op == BinOp::Lt {
                    self.close_cast_before_less_than(&mut binop.left);
                }
                self.inline_operand(&mut binop.left, OperandPosition::Left(binop.op));
                self.inline_operand(&mut binop.right, OperandPosition::Right(binop.op));
            }
            Expression::UnaryOp(unop) => {
                self.inline_operand(&mut unop.operand, OperandPosition::Unary);
            }
            // Luau
            Expression::TypeCast(cast) => {
                self.inline_operand(&mut cast.expr, OperandPosition::Cast);
            }
            // These delimit their children or are leaves. A literal landing in
            // a prefix position is `ensure_prefix`'s job.
            Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::Nil(_)
            | Expression::True(_)
            | Expression::False(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Var(_)
            | Expression::FunctionCall(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_) => {}
        }
    }

    fn inline_operand(&self, operand: &mut Expression, position: OperandPosition) {
        if let Expression::Var(Var::Name(name)) = operand
            && let Some(initializer) = self.candidates.get(identifier_name(name))
        {
            let replacement = initializer.clone();
            *operand = if keeps_grouping(&replacement, position) {
                replacement
            } else {
                parenthesize(replacement)
            };
        }
    }

    /// Luau: a cast has no closing token and `T<` opens generic arguments, so a
    /// cast inlined just before `<` needs parens of its own. A cast that was
    /// already there parsed fine, so only inlining can cause this.
    fn close_cast_before_less_than(&mut self, left: &mut Expression) {
        if let Expression::Var(Var::Name(name)) = last_operand(left)
            && let Some(initializer) = self.candidates.get_mut(identifier_name(name))
        {
            let cast = last_operand(initializer);
            if matches!(cast, Expression::TypeCast(_)) {
                let placeholder = Expression::Nil(Span::default());
                *cast = parenthesize(mem::replace(cast, placeholder));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    fn apply_version(source: &str, version: LuaVersion) -> String {
        run_pass(source, version, |block| inline(block, version))
    }

    #[test]
    fn inlines_single_use_number() {
        let result = apply("local x = 42\nreturn x\n");
        assert!(!result.contains("local"), "Declaration not removed: {result}");
        assert!(result.contains("42"), "Value not inlined: {result}");
    }

    #[test]
    fn no_inline_of_multi_use_local() {
        let result = apply("local x = 42\nprint(x)\nreturn x\n");
        assert!(result.contains("local"), "Multi-use local was inlined: {result}");
    }

    #[test]
    fn no_inline_of_call_initializer() {
        let result = apply("local x = foo()\nreturn x\n");
        assert!(result.contains("local"), "Side-effectful init was inlined: {result}");
    }

    #[test]
    fn no_inline_of_variable_read() {
        let result = apply("local x = y\nreturn x\n");
        assert!(result.contains("local"), "Variable read was inlined: {result}");
    }

    #[test]
    fn inlines_in_nested_function_body() {
        let result = apply("local function foo()\n  local x = 42\n  return x\nend\nreturn foo\n");
        assert!(
            !result.contains("local x"),
            "Declaration inside function body not inlined: {result}"
        );
    }

    #[test]
    fn inlines_in_if_block() {
        let result = apply("if true then\n  local x = 42\n  print(x)\nend\n");
        assert!(!result.contains("local x"), "Declaration inside if block not inlined: {result}");
    }

    #[test]
    fn parenthesizes_literal_inlined_into_index_prefix() {
        let result = apply("local x = 37.5\nreturn x[\"k\"]\n");
        assert!(
            result.contains("(37.5)[\"k\"]"),
            "Literal in index-prefix position must gain parens: {result}"
        );
    }

    #[test]
    fn parenthesizes_literal_inlined_into_field_prefix() {
        let result = apply("local x = \"s\"\nreturn x.len\n");
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
            ("local s = \"x\"\nreturn s<<T>>\n", "return(\"x\")<<T>>"),
            ("local s = 1\nreturn s<<T>>\n", "return(1)<<T>>"),
            ("local s = nil\nreturn s<<T>>\n", "return(nil)<<T>>"),
            ("local s = -1\nreturn s<<T>>\n", "return(-1)<<T>>"),
            ("local s = \"x\" :: T\nreturn s<<U>>\n", "return(\"x\"::T)<<U>>"),
            ("local s = \"x\"\nreturn s<<T>><<U>>\n", "return(\"x\")<<T>><<U>>"),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn literal_inlined_under_instantiation_reparses_as_prefix() {
        apply_luau_reparsing("local s = \"x\"\nreturn s<<T>>()\n");
        apply_luau_reparsing("local s = \"x\"\nreturn s<<T>>.y\n");
        apply_luau_reparsing("local s = \"x\"\ns<<T>>()\n");
    }

    #[test]
    fn leaves_chained_type_instantiation_unwrapped() {
        // The instantiation guard must not parenthesize a chain that is already
        // a valid prefix, whether the chain sits on a var or an inlined prefix.
        for (source, expected) in [
            ("local s = 1\nprint(s)\nreturn f<<T>><<U>>\n", "print(1)return f<<T>><<U>>"),
            ("local s = (\"x\")<<T>>\nreturn s<<U>>\n", "return(\"x\")<<T>><<U>>"),
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
            ("local s = 1 / 0\nreturn 3 % s\n", "return 3%(1/0)"),
            ("local s = 1 + 2\nreturn s * 2\n", "return(1+2)*2"),
            ("local s = 1 + 2\nreturn 2 * s\n", "return 2*(1+2)"),
            ("local s = 1 - 2\nreturn 5 - s\n", "return 5-(1-2)"),
            ("local s = 2 ^ 3\nreturn s ^ 2\n", "return(2^3)^2"),
            ("local s = 1 .. 2\nreturn s .. 3\n", "return(1 ..2)..3"),
            ("local s = 1 + 2\nreturn 2 ^ s\n", "return 2^(1+2)"),
            ("local s = 1 | 2\nreturn s & 3\n", "return(1|2)&3"),
            ("local s = 1 or 2\nreturn s and 3\n", "return(1 or 2)and 3"),
            ("local s = -2\nreturn s ^ y\n", "return(-2)^y"),
            ("local s = not nil\nreturn s ^ y\n", "return(not nil)^y"),
        ]);
    }

    #[test]
    fn parenthesizes_operator_inlined_under_a_unary() {
        assert_inlines(&[
            ("local s = 1 + 2\nreturn -s\n", "return-(1+2)"),
            ("local s = 1 == 2\nreturn not s\n", "return not(1==2)"),
            ("local s = \"a\" .. \"b\"\nreturn #s\n", "return#(\"a\"..\"b\")"),
            ("local s = 1 * 2\nreturn ~s\n", "return~(1*2)"),
        ]);
    }

    #[test]
    fn parenthesizes_operator_inlined_into_a_nested_operand() {
        assert_inlines(&[
            ("local s = 1 + 2\nreturn -s ^ 2\n", "return-(1+2)^2"),
            ("local s = -1\nreturn -s ^ 2\n", "return-(-1)^2"),
            ("local s = 1 + 2\nreturn (s * 3)\n", "return((1+2)*3)"),
            ("local s = 1 + 2\nprint(s * 3, 4)\n", "print((1+2)*3,4)"),
            ("local s = 1 + 2\nreturn {s * 3}\n", "return{(1+2)*3}"),
            ("local s = 1 + 2\nreturn t[s * 3]\n", "return t[(1+2)*3]"),
            ("local s = 1 + 2\nif s * 3 then end\n", "if(1+2)*3 then end"),
            ("local a = 1 + 2\nlocal b = a * 3\nreturn b ^ 2\n", "return((1+2)*3)^2"),
        ]);
    }

    #[test]
    fn leaves_inlined_expression_bare_where_its_grouping_holds() {
        assert_inlines(&[
            ("local s = 1 + 2\nreturn s\n", "return 1+2"),
            ("local s = 1 + 2\nprint(s)\n", "print(1+2)"),
            ("local s = 1 + 2\nreturn {s}\n", "return{1+2}"),
            ("local s = 1 + 2\nreturn t[s]\n", "return t[1+2]"),
            ("local s = 1 * 2\nreturn s + 3\n", "return 1*2+3"),
            ("local s = 1 * 2\nreturn 3 + s\n", "return 3+1*2"),
            ("local s = 1 - 2\nreturn s - 5\n", "return 1-2-5"),
            ("local s = 2 ^ 3\nreturn 2 ^ s\n", "return 2^2^3"),
            ("local s = 1 .. 2\nreturn 3 .. s\n", "return 3 ..1 ..2"),
            ("local s = 1 + 2\nreturn s < 4\n", "return 1+2<4"),
            ("local s = 1 < 2\nreturn s == true\n", "return 1<2==true"),
            ("local s = 2 ^ 3\nreturn -s\n", "return-2^3"),
            ("local s = -2\nreturn 3 ^ s\n", "return 3^-2"),
            ("local s = -2\nreturn s * 3\n", "return-2*3"),
            ("local s = -1\nreturn -s\n", "return- -1"),
            ("local s = (1 + 2)\nreturn s * 2\n", "return(1+2)*2"),
            ("local s = 2\nreturn s * 3\n", "return 2*3"),
        ]);
    }

    #[test]
    fn parenthesizes_expression_inlined_under_a_cast() {
        // `::` binds to a simple expression and doesn't chain.
        for (source, expected) in [
            ("local s = 1 + 2\nreturn s :: any\n", "return(1+2)::any"),
            ("local s = -1\nreturn s :: any\n", "return(-1)::any"),
            ("local s = 1 :: number\nreturn s :: any\n", "return(1::number)::any"),
            ("local s = 1\nreturn s :: any\n", "return 1::any"),
            ("local s = (1 + 2)\nreturn s :: any\n", "return(1+2)::any"),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn parenthesizes_cast_inlined_before_less_than() {
        // `T<` opens generic arguments, so the cast has to end before the `<`.
        for (source, expected) in [
            ("local s = 1 :: number\nreturn s < 2\n", "return(1::number)<2"),
            ("local s = 1 :: number\nreturn 2 + s < 3\n", "return 2+(1::number)<3"),
            ("local s = 1 :: number\nreturn -s < 2\n", "return-(1::number)<2"),
            ("local s = 1 :: number\nreturn 1 < s < 3\n", "return 1<(1::number)<3"),
            ("local s = 2 + 1 :: number\nreturn s < 3\n", "return 2+(1::number)<3"),
            ("local s = -1 :: number\nreturn s < 3\n", "return-(1::number)<3"),
            ("local s = 2 + 1 :: number\nreturn 2 * s < 3\n", "return 2*(2+(1::number))<3"),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn leaves_cast_bare_where_nothing_extends_its_type() {
        for (source, expected) in [
            ("local s = 1 :: number\nreturn s\n", "return 1::number"),
            ("local s = 1 :: number\nreturn s + 1\n", "return 1::number+1"),
            ("local s = 1 :: number\nreturn 1 + s\n", "return 1+1::number"),
            ("local s = 1 :: number\nreturn -s\n", "return-1::number"),
            ("local s = 1 :: number\nreturn s > 2\n", "return 1::number>2"),
            ("local s = 1 :: number\nreturn 2 < s\n", "return 2<1::number"),
            ("local s = 1\nreturn x :: Foo<T> < s\n", "return x::Foo<T><1"),
            ("local s = (\"x\")<<T>>\nreturn s .. \"y\"\n", "return(\"x\")<<T>>..\"y\""),
        ] {
            assert_eq!(apply_luau_reparsing(source), expected, "source: {source}");
        }
    }

    #[test]
    fn parenthesizes_literal_inlined_into_statement_call_receiver() {
        let result = apply("local x = false\nx:resize(9)\n");
        assert!(
            result.contains("(false):resize(9)"),
            "Literal receiver of a statement call must gain parens: {result}"
        );
    }

    #[test]
    fn no_inline_of_table_constructor() {
        // Tables carry identity: moved into a loop, `{1,2,3}` would mint
        // a fresh table per iteration.
        let result = apply("local t = {1, 2, 3}\nreturn t\n");
        assert!(result.contains("local"), "Identity value was inlined: {result}");
    }

    #[test]
    fn no_inline_of_function_def() {
        // Closures carry identity and captures, and both depend on position:
        // in `local a=1 local f=function() return a end local a=2`, moving f's
        // body past the second `a` would rebind it.
        let result = apply("local f = function() return 1 end\nreturn f\n");
        assert!(result.contains("local"), "Closure was inlined: {result}");
    }

    #[test]
    fn inlines_chained_single_use() {
        // Pass 1 inlines x into y's initializer, giving `local y = 42 + 1`.
        // Pass 2 finds that closed literal and inlines y, giving
        // `return 42 + 1`.
        let result = apply("local x = 42\nlocal y = x + 1\nreturn y\n");
        assert!(!result.contains("local"), "Both should be inlined: {result}");
    }

    #[test]
    fn no_inline_of_vararg() {
        // Inside a nested function, `...` is that function's own varargs, or
        // a compile error if it takes none.
        let result = apply("local function f(...)\n  local x = ...\n  return x\nend\n");
        assert!(result.contains("local x"), "VarArg was inlined: {result}");
    }

    #[test]
    fn no_inline_of_multi_use_vararg() {
        let result = apply("local function f(...)\n  local x = ...\n  print(x)\n  return x\nend\n");
        assert!(result.contains("local x"), "Multi-use vararg should not be inlined: {result}");
    }

    #[test]
    fn no_inline_of_variable_read_under_cast() {
        // TypeCast is transparent, and the inner variable read is impure.
        let result = apply_version("local x = foo :: Bar\nreturn x\n", LuaVersion::Luau);
        assert!(result.contains("local"), "TypeCast with var read should not be inlined: {result}");
    }

    #[test]
    fn no_inline_across_global_function_shadow() {
        // The outer `x` is never read, and the `global function` body
        // declares its own `x` and reads it once. The scan has to see that
        // second declaration, or the outer value leaks in as
        // `global function g()return 1 end`.
        let result = apply_version(
            "local x = 1\nglobal function g()\n  local x = 2\n  return x\nend\nreturn g\n",
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
}
