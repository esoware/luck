//! Structural AST equivalence for `luck fmt --verify` and
//! `format_and_verify`.
//!
//! Spans, comments, and trivia are ignored. Shape, names, and literal values
//! have to match, apart from the rewrites the formatter is allowed to make,
//! such as dropping redundant parens or swapping quote style.
//!
//! A divergence starts with an empty path, and each level it unwinds through
//! prepends its own segment, so a comparison that matches builds no path.

use core::ops::ControlFlow::{self, Break, Continue};

use luck_ast::{
    Assignment, AttributedName, Block, CompoundAssignment, ElseIfClause, ElseIfExprClause,
    Expression, Field, FunctionArgs, FunctionAttribute, FunctionBody, FunctionCall, FunctionDecl,
    FunctionDef, FunctionType, FunctionTypeParam, GenericFor, GenericTypeList, GenericTypeParam,
    GlobalDeclaration, IfExpression, IfStatement, LastStatement, LocalAssignment, LocalFunction,
    NamedType, NumericFor, Parameter, Punctuated, Statement, TableConstructor, TableType, Type,
    TypeArgs, TypeDeclaration, TypeDeclarationValue, TypeField, TypeInstantiation, Var,
};
use luck_token::{LuaVersion, Token, TokenKind, decode_string};

/// First point of divergence between two ASTs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstDiff {
    /// Slash-separated path into the tree, e.g.
    /// `block/stmts[3]/FunctionDecl/body`.
    pub path: String,
    pub reason: String,
}

impl AstDiff {
    pub(crate) fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self { path: path.into(), reason: reason.into() }
    }

    /// This diff as seen from the node that holds `segment`.
    fn within(mut self, segment: &str) -> Self {
        self.path = if self.path.is_empty() {
            segment.to_owned()
        } else {
            format!("{segment}/{}", self.path)
        };
        self
    }
}

/// A divergence at the node being compared.
fn differ(reason: impl Into<String>) -> ControlFlow<AstDiff> {
    Break(AstDiff::new(String::new(), reason))
}

/// `check`, which compared the children at `segment`.
fn at(segment: &str, check: ControlFlow<AstDiff>) -> ControlFlow<AstDiff> {
    check.map_break(|diff| diff.within(segment))
}

/// `check`, which compared child `index` of the list `name`.
fn at_index(name: &str, index: usize, check: ControlFlow<AstDiff>) -> ControlFlow<AstDiff> {
    check.map_break(|diff| diff.within(&format!("{name}[{index}]")))
}

/// Continue when `is_satisfied`, and diverge for `reason` otherwise.
fn require(is_satisfied: bool, reason: &str) -> ControlFlow<AstDiff> {
    if is_satisfied { Continue(()) } else { differ(reason) }
}

/// Both absent, or both present and equivalent under `item_eq`.
fn presence_eq<T>(
    left: Option<&T>,
    right: Option<&T>,
    presence_reason: &str,
    item_eq: impl FnOnce(&T, &T) -> ControlFlow<AstDiff>,
) -> ControlFlow<AstDiff>
where
    T: ?Sized,
{
    match (left, right) {
        (None, None) => Continue(()),
        (Some(lhs), Some(rhs)) => item_eq(lhs, rhs),
        (Some(_), None) | (None, Some(_)) => differ(presence_reason),
    }
}

/// Compare two blocks parsed as `version` structurally.
///
/// String literals compare by the bytes they decode to in that dialect.
///
/// # Errors
///
/// Returns the first point where the two blocks diverge.
pub fn blocks_equivalent(left: &Block, right: &Block, version: LuaVersion) -> Result<(), AstDiff> {
    match at("block", block_eq(left, right, version)) {
        Continue(()) => Ok(()),
        Break(diff) => Err(diff),
    }
}

fn block_eq(left: &Block, right: &Block, version: LuaVersion) -> ControlFlow<AstDiff> {
    // The formatter drops `;` empty statements and adds one before a
    // statement starting with `(`, so empty statements don't count.
    let is_counted = |stmt: &&Statement| !matches!(stmt, Statement::EmptyStatement(_));
    let left_count = left.stmts.iter().filter(is_counted).count();
    let right_count = right.stmts.iter().filter(is_counted).count();

    // Verification compares against the buffer `sort_requires` already
    // reordered, so statements still match one to one.
    if left_count != right_count {
        return differ(format!("statement count differs: {left_count} vs {right_count}"));
    }
    let left_stmts = left.stmts.iter().filter(is_counted);
    let right_stmts = right.stmts.iter().filter(is_counted);
    for (index, (lhs, rhs)) in left_stmts.zip(right_stmts).enumerate() {
        at_index("stmts", index, stmt_eq(lhs, rhs, version))?;
    }
    presence_eq(
        left.last_stmt.as_deref(),
        right.last_stmt.as_deref(),
        "last statement presence differs",
        |lhs, rhs| at("last_stmt", last_stmt_eq(lhs, rhs, version)),
    )
}

fn stmt_eq(left: &Statement, right: &Statement, version: LuaVersion) -> ControlFlow<AstDiff> {
    match (left, right) {
        (Statement::Assignment(lhs), Statement::Assignment(rhs)) => {
            assignment_eq(lhs, rhs, version)
        }
        (Statement::FunctionCall(lhs), Statement::FunctionCall(rhs)) => {
            at("FunctionCall", function_call_eq(lhs, rhs, version))
        }
        (Statement::DoBlock(lhs), Statement::DoBlock(rhs)) => {
            at("DoBlock", block_eq(&lhs.block, &rhs.block, version))
        }
        (Statement::WhileLoop(lhs), Statement::WhileLoop(rhs)) => {
            at("WhileLoop/condition", expr_eq(&lhs.condition, &rhs.condition, version))?;
            at("WhileLoop/block", block_eq(&lhs.block, &rhs.block, version))
        }
        (Statement::RepeatLoop(lhs), Statement::RepeatLoop(rhs)) => {
            at("RepeatLoop/block", block_eq(&lhs.block, &rhs.block, version))?;
            at("RepeatLoop/condition", expr_eq(&lhs.condition, &rhs.condition, version))
        }
        (Statement::IfStatement(lhs), Statement::IfStatement(rhs)) => {
            if_statement_eq(lhs, rhs, version)
        }
        (Statement::NumericFor(lhs), Statement::NumericFor(rhs)) => {
            numeric_for_eq(lhs, rhs, version)
        }
        (Statement::GenericFor(lhs), Statement::GenericFor(rhs)) => {
            generic_for_eq(lhs, rhs, version)
        }
        (Statement::FunctionDecl(lhs), Statement::FunctionDecl(rhs)) => {
            function_decl_eq(lhs, rhs, version)
        }
        (Statement::LocalFunction(lhs), Statement::LocalFunction(rhs)) => {
            local_function_eq(lhs, rhs, version)
        }
        (Statement::GlobalFunction(lhs), Statement::GlobalFunction(rhs)) => {
            at("GlobalFunction/name", token_text_eq(&lhs.name, &rhs.name))?;
            at("GlobalFunction/body", function_body_eq(&lhs.body, &rhs.body, version))
        }
        (Statement::LocalAssignment(lhs), Statement::LocalAssignment(rhs)) => {
            local_assignment_eq(lhs, rhs, version)
        }
        (Statement::EmptyStatement(_), Statement::EmptyStatement(_))
        | (Statement::GlobalStar(_), Statement::GlobalStar(_))
        | (Statement::Break(_), Statement::Break(_))
        | (Statement::Error(_), Statement::Error(_)) => Continue(()),
        (Statement::Goto(lhs), Statement::Goto(rhs)) => {
            at("Goto/name", token_text_eq(&lhs.name, &rhs.name))
        }
        (Statement::Label(lhs), Statement::Label(rhs)) => {
            at("Label/name", token_text_eq(&lhs.name, &rhs.name))
        }
        (Statement::GlobalDeclaration(lhs), Statement::GlobalDeclaration(rhs)) => {
            global_declaration_eq(lhs, rhs, version)
        }
        (Statement::CompoundAssignment(lhs), Statement::CompoundAssignment(rhs)) => {
            compound_assignment_eq(lhs, rhs, version)
        }
        (Statement::TypeDeclaration(lhs), Statement::TypeDeclaration(rhs)) => {
            type_declaration_eq(lhs, rhs, version)
        }
        (
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::GlobalFunction(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_),
            _,
        ) => differ(format!(
            "statement kind differs: `{}` vs `{}`",
            stmt_kind(left),
            stmt_kind(right)
        )),
    }
}

fn assignment_eq(
    left: &Assignment,
    right: &Assignment,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("Assignment/targets", punctuated_eq(&left.targets, &right.targets, version, var_eq))?;
    at("Assignment/values", punctuated_eq(&left.values, &right.values, version, expr_eq))
}

fn compound_assignment_eq(
    left: &CompoundAssignment,
    right: &CompoundAssignment,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("CompoundAssignment/var", var_eq(&left.var, &right.var, version))?;
    at("CompoundAssignment/expr", expr_eq(&left.expr, &right.expr, version))
}

fn if_statement_eq(
    left: &IfStatement,
    right: &IfStatement,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("IfStatement/condition", expr_eq(&left.condition, &right.condition, version))?;
    at("IfStatement/block", block_eq(&left.block, &right.block, version))?;
    require(left.elseif_clauses.len() == right.elseif_clauses.len(), "`elseif` count differs")?;
    let clause_eq = |lhs: &ElseIfClause, rhs: &ElseIfClause| {
        at("condition", expr_eq(&lhs.condition, &rhs.condition, version))?;
        at("block", block_eq(&lhs.block, &rhs.block, version))
    };
    let clauses = left.elseif_clauses.iter().zip(&right.elseif_clauses);
    for (index, (lhs, rhs)) in clauses.enumerate() {
        at_index("elseif_clauses", index, clause_eq(lhs, rhs))?;
    }
    presence_eq(
        left.else_clause.as_ref(),
        right.else_clause.as_ref(),
        "`else` presence differs",
        |lhs, rhs| at("else_clause", block_eq(&lhs.block, &rhs.block, version)),
    )
}

fn numeric_for_eq(
    left: &NumericFor,
    right: &NumericFor,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("NumericFor/name", token_text_eq(&left.name, &right.name))?;
    at(
        "NumericFor/type_annotation",
        type_annotation_eq(left.type_annotation.as_ref(), right.type_annotation.as_ref(), version),
    )?;
    at("NumericFor/start", expr_eq(&left.start, &right.start, version))?;
    at("NumericFor/limit", expr_eq(&left.limit, &right.limit, version))?;
    presence_eq(
        left.step.as_ref(),
        right.step.as_ref(),
        "`NumericFor` step presence differs",
        |lhs, rhs| at("NumericFor/step", expr_eq(lhs, rhs, version)),
    )?;
    at("NumericFor/block", block_eq(&left.block, &right.block, version))
}

fn generic_for_eq(
    left: &GenericFor,
    right: &GenericFor,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("GenericFor/names", punctuated_eq(&left.names, &right.names, version, param_eq))?;
    at("GenericFor/exprs", punctuated_eq(&left.exprs, &right.exprs, version, expr_eq))?;
    at("GenericFor/block", block_eq(&left.block, &right.block, version))
}

fn function_decl_eq(
    left: &FunctionDecl,
    right: &FunctionDecl,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    require(left.name.names.len() == right.name.names.len(), "`FunctionDecl` name length differs")?;
    for (index, (lhs, rhs)) in left.name.names.iter().zip(&right.name.names).enumerate() {
        at_index("FunctionDecl/name/names", index, token_text_eq(lhs, rhs))?;
    }
    presence_eq(
        left.name.method.as_ref(),
        right.name.method.as_ref(),
        "`FunctionDecl` method presence differs",
        |lhs, rhs| at("FunctionDecl/name/method", token_text_eq(lhs, rhs)),
    )?;
    at("FunctionDecl", function_attributes_eq(&left.attributes, &right.attributes, version))?;
    at("FunctionDecl/body", function_body_eq(&left.body, &right.body, version))
}

fn local_function_eq(
    left: &LocalFunction,
    right: &LocalFunction,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("LocalFunction/name", token_text_eq(&left.name, &right.name))?;
    require(left.is_const == right.is_const, "`LocalFunction` const-ness differs")?;
    require(left.is_exported == right.is_exported, "`LocalFunction` export differs")?;
    at("LocalFunction", function_attributes_eq(&left.attributes, &right.attributes, version))?;
    at("LocalFunction/body", function_body_eq(&left.body, &right.body, version))
}

fn local_assignment_eq(
    left: &LocalAssignment,
    right: &LocalAssignment,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    require(left.is_const == right.is_const, "`LocalAssignment` const-ness differs")?;
    require(left.is_exported == right.is_exported, "`LocalAssignment` export differs")?;
    at(
        "LocalAssignment/names",
        punctuated_eq(&left.names, &right.names, version, attributed_name_eq),
    )?;
    presence_eq(
        left.exprs.as_ref(),
        right.exprs.as_ref(),
        "`LocalAssignment` exprs presence differs",
        |lhs, rhs| at("LocalAssignment/exprs", punctuated_eq(lhs, rhs, version, expr_eq)),
    )
}

fn global_declaration_eq(
    left: &GlobalDeclaration,
    right: &GlobalDeclaration,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at(
        "GlobalDeclaration/names",
        punctuated_eq(&left.names, &right.names, version, attributed_name_eq),
    )?;
    presence_eq(
        left.exprs.as_ref(),
        right.exprs.as_ref(),
        "`GlobalDeclaration` exprs presence differs",
        |lhs, rhs| at("GlobalDeclaration/exprs", punctuated_eq(lhs, rhs, version, expr_eq)),
    )
}

fn type_declaration_eq(
    left: &TypeDeclaration,
    right: &TypeDeclaration,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("TypeDeclaration/name", token_text_eq(&left.name, &right.name))?;
    at(
        "TypeDeclaration/generics",
        generics_eq(left.generics.as_deref(), right.generics.as_deref(), version),
    )?;
    match (&left.type_value, &right.type_value) {
        (TypeDeclarationValue::Alias(lhs), TypeDeclarationValue::Alias(rhs)) => {
            at("TypeDeclaration/type_value", type_eq(lhs, rhs, version))
        }
        (TypeDeclarationValue::TypeFunction(lhs), TypeDeclarationValue::TypeFunction(rhs)) => {
            at("TypeDeclaration/type_value", function_body_eq(lhs, rhs, version))
        }
        (TypeDeclarationValue::Alias(_) | TypeDeclarationValue::TypeFunction(_), _) => {
            differ("type declaration value kind differs")
        }
    }
}

fn last_stmt_eq(
    left: &LastStatement,
    right: &LastStatement,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    match (left, right) {
        (LastStatement::Return(lhs), LastStatement::Return(rhs)) => {
            at("Return/exprs", punctuated_eq(&lhs.exprs, &rhs.exprs, version, expr_eq))
        }
        (LastStatement::Break(_), LastStatement::Break(_))
        | (LastStatement::Continue(_), LastStatement::Continue(_))
        | (LastStatement::Error(_), LastStatement::Error(_)) => Continue(()),
        (
            LastStatement::Return(_)
            | LastStatement::Break(_)
            | LastStatement::Continue(_)
            | LastStatement::Error(_),
            _,
        ) => differ("last statement kind differs"),
    }
}

fn expr_eq(left: &Expression, right: &Expression, version: LuaVersion) -> ControlFlow<AstDiff> {
    match (left, right) {
        // Interpolated strings always match. Their embedded expressions
        // aren't compared, even though the formatter reformats them.
        (Expression::Nil(_), Expression::Nil(_))
        | (Expression::False(_), Expression::False(_))
        | (Expression::True(_), Expression::True(_))
        | (Expression::VarArg(_), Expression::VarArg(_))
        | (Expression::InterpolatedString(_), Expression::InterpolatedString(_))
        | (Expression::Error(_), Expression::Error(_)) => Continue(()),
        (Expression::StringLiteral(lhs), Expression::StringLiteral(rhs)) => {
            // The formatter may swap quote style, so compare decoded contents.
            require(string_raw_eq(&lhs.text, &rhs.text, version), "literal contents differ")
        }
        (Expression::Number(lhs), Expression::Number(rhs)) => {
            require(number_raw_eq(&lhs.text, &rhs.text), "number literals differ")
        }
        (Expression::Integer(lhs), Expression::Integer(rhs)) => {
            require(number_raw_eq(&lhs.text, &rhs.text), "integer literals differ")
        }
        (Expression::Var(lhs), Expression::Var(rhs)) => var_eq(lhs, rhs, version),
        (Expression::BinaryOp(lhs), Expression::BinaryOp(rhs)) => {
            require(lhs.op == rhs.op, "binary op differs")?;
            at("left", expr_eq(&lhs.left, &rhs.left, version))?;
            at("right", expr_eq(&lhs.right, &rhs.right, version))
        }
        (Expression::UnaryOp(lhs), Expression::UnaryOp(rhs)) => {
            require(lhs.op == rhs.op, "unary op differs")?;
            at("operand", expr_eq(&lhs.operand, &rhs.operand, version))
        }
        (Expression::Parenthesized(lhs), Expression::Parenthesized(rhs)) => {
            at("Parenthesized/expr", expr_eq(&lhs.expr, &rhs.expr, version))
        }
        // The formatter may add or drop redundant parens, so `(x)` matches
        // `x` however deep the nesting.
        (Expression::Parenthesized(lhs), other) => expr_eq(&lhs.expr, other, version),
        (other, Expression::Parenthesized(rhs)) => expr_eq(other, &rhs.expr, version),
        (Expression::FunctionCall(lhs), Expression::FunctionCall(rhs)) => {
            at("FunctionCall", function_call_eq(lhs, rhs, version))
        }
        (Expression::FunctionDef(lhs), Expression::FunctionDef(rhs)) => {
            function_def_eq(lhs, rhs, version)
        }
        (Expression::TableConstructor(lhs), Expression::TableConstructor(rhs)) => {
            at("TableConstructor", table_eq(lhs, rhs, version))
        }
        (Expression::IfExpression(lhs), Expression::IfExpression(rhs)) => {
            if_expression_eq(lhs, rhs, version)
        }
        (Expression::TypeCast(lhs), Expression::TypeCast(rhs)) => {
            at("TypeCast/expr", expr_eq(&lhs.expr, &rhs.expr, version))?;
            at(
                "TypeCast/type_annotation",
                type_eq(&lhs.type_annotation, &rhs.type_annotation, version),
            )
        }
        (Expression::TypeInstantiation(lhs), Expression::TypeInstantiation(rhs)) => {
            type_instantiation_eq(lhs, rhs, version)
        }
        (
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::VarArg(_)
            | Expression::InterpolatedString(_)
            | Expression::Error(_)
            | Expression::StringLiteral(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::Var(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::FunctionCall(_)
            | Expression::FunctionDef(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_),
            _,
        ) => differ(format!(
            "expression kind differs: `{}` vs `{}`",
            expr_kind(left),
            expr_kind(right)
        )),
    }
}

fn function_def_eq(
    left: &FunctionDef,
    right: &FunctionDef,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("FunctionDef", function_attributes_eq(&left.attributes, &right.attributes, version))?;
    at("FunctionDef/body", function_body_eq(&left.body, &right.body, version))
}

/// Luau `f<<T>>`.
fn type_instantiation_eq(
    left: &TypeInstantiation,
    right: &TypeInstantiation,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("TypeInstantiation/expr", expr_eq(&left.expr, &right.expr, version))?;
    at(
        "TypeInstantiation/type_args",
        punctuated_eq(&left.type_args.args, &right.type_args.args, version, type_eq),
    )
}

fn if_expression_eq(
    left: &IfExpression,
    right: &IfExpression,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("IfExpression/condition", expr_eq(&left.condition, &right.condition, version))?;
    at("IfExpression/then_expr", expr_eq(&left.then_expr, &right.then_expr, version))?;
    require(
        left.elseif_clauses.len() == right.elseif_clauses.len(),
        "if-expression `elseif` count differs",
    )?;
    let clause_eq = |lhs: &ElseIfExprClause, rhs: &ElseIfExprClause| {
        at("condition", expr_eq(&lhs.condition, &rhs.condition, version))?;
        at("expr", expr_eq(&lhs.expr, &rhs.expr, version))
    };
    let clauses = left.elseif_clauses.iter().zip(&right.elseif_clauses);
    for (index, (lhs, rhs)) in clauses.enumerate() {
        at_index("IfExpression/elseif_clauses", index, clause_eq(lhs, rhs))?;
    }
    at("IfExpression/else_expr", expr_eq(&left.else_expr, &right.else_expr, version))
}

fn var_eq(left: &Var, right: &Var, version: LuaVersion) -> ControlFlow<AstDiff> {
    match (left, right) {
        (Var::Name(lhs), Var::Name(rhs)) => token_text_eq(lhs, rhs),
        (Var::FieldAccess(lhs), Var::FieldAccess(rhs)) => {
            at("prefix", expr_eq(&lhs.prefix, &rhs.prefix, version))?;
            at("name", token_text_eq(&lhs.name, &rhs.name))
        }
        (Var::Index(lhs), Var::Index(rhs)) => {
            at("prefix", expr_eq(&lhs.prefix, &rhs.prefix, version))?;
            at("index", expr_eq(&lhs.index, &rhs.index, version))
        }
        (Var::Name(_) | Var::FieldAccess(_) | Var::Index(_), _) => differ("var kind differs"),
    }
}

fn function_call_eq(
    left: &FunctionCall,
    right: &FunctionCall,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("callee", expr_eq(&left.callee, &right.callee, version))?;
    presence_eq(
        left.method.as_ref(),
        right.method.as_ref(),
        "method presence differs",
        |lhs, rhs| at("method", token_text_eq(lhs, rhs)),
    )?;
    presence_eq(
        left.explicit_type_args.as_deref(),
        right.explicit_type_args.as_deref(),
        "explicit type argument presence differs",
        |lhs, rhs| at("explicit_type_args", punctuated_eq(&lhs.args, &rhs.args, version, type_eq)),
    )?;
    at("args", args_eq(&left.args, &right.args, version))
}

fn args_eq(left: &FunctionArgs, right: &FunctionArgs, version: LuaVersion) -> ControlFlow<AstDiff> {
    match (left, right) {
        (
            FunctionArgs::Parenthesized { args: lhs, .. },
            FunctionArgs::Parenthesized { args: rhs, .. },
        ) => punctuated_eq(lhs, rhs, version, expr_eq),
        (FunctionArgs::TableConstructor(lhs), FunctionArgs::TableConstructor(rhs)) => {
            table_eq(lhs, rhs, version)
        }
        (FunctionArgs::StringLiteral(lhs), FunctionArgs::StringLiteral(rhs)) => {
            require(string_raw_eq(&lhs.text, &rhs.text, version), "call string arg differs")
        }
        // The formatter may add or drop the parens in `f("x")` and `f({1})`.
        (FunctionArgs::StringLiteral(literal), FunctionArgs::Parenthesized { args, .. })
        | (FunctionArgs::Parenthesized { args, .. }, FunctionArgs::StringLiteral(literal)) => {
            if args.len() == 1
                && let Some(Expression::StringLiteral(other)) = args.first()
                && string_raw_eq(&literal.text, &other.text, version)
            {
                return Continue(());
            }
            differ("call arg shape differs")
        }
        (FunctionArgs::TableConstructor(table), FunctionArgs::Parenthesized { args, .. })
        | (FunctionArgs::Parenthesized { args, .. }, FunctionArgs::TableConstructor(table)) => {
            if args.len() == 1
                && let Some(Expression::TableConstructor(other)) = args.first()
            {
                return table_eq(table, other, version);
            }
            differ("call arg shape differs")
        }
        (FunctionArgs::StringLiteral(_), FunctionArgs::TableConstructor(_))
        | (FunctionArgs::TableConstructor(_), FunctionArgs::StringLiteral(_)) => {
            differ("call arg shape differs")
        }
    }
}

/// Attributes have to match in order, by name and arguments, since losing one
/// such as `@native` changes how the function is compiled.
fn function_attributes_eq(
    left: &[FunctionAttribute],
    right: &[FunctionAttribute],
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    require(left.len() == right.len(), "attribute count differs")?;
    for (index, (lhs, rhs)) in left.iter().zip(right).enumerate() {
        at_index("attributes", index, token_text_eq(&lhs.name, &rhs.name))?;
        presence_eq(
            lhs.args.as_ref(),
            rhs.args.as_ref(),
            "attribute args presence differs",
            |left_args, right_args| {
                at_index(
                    "attributes",
                    index,
                    at("args", punctuated_eq(left_args, right_args, version, expr_eq)),
                )
            },
        )?;
    }
    Continue(())
}

fn function_body_eq(
    left: &FunctionBody,
    right: &FunctionBody,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("generics", generics_eq(left.generics.as_deref(), right.generics.as_deref(), version))?;
    at("params", punctuated_eq(&left.params, &right.params, version, param_eq))?;
    presence_eq(
        left.vararg.as_deref(),
        right.vararg.as_deref(),
        "vararg presence differs",
        |lhs, rhs| {
            at(
                "vararg/type_annotation",
                type_annotation_eq(
                    lhs.type_annotation.as_ref(),
                    rhs.type_annotation.as_ref(),
                    version,
                ),
            )
        },
    )?;
    at(
        "return_type",
        type_annotation_eq(left.return_type.as_ref(), right.return_type.as_ref(), version),
    )?;
    at("block", block_eq(&left.block, &right.block, version))
}

fn param_eq(left: &Parameter, right: &Parameter, version: LuaVersion) -> ControlFlow<AstDiff> {
    token_text_eq(&left.name, &right.name)?;
    at(
        "type_annotation",
        type_annotation_eq(left.type_annotation.as_ref(), right.type_annotation.as_ref(), version),
    )
}

/// Compare optional `: Type` annotations. A dropped annotation is data loss,
/// so presence has to match too.
fn type_annotation_eq(
    left: Option<&Type>,
    right: Option<&Type>,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    presence_eq(left, right, "type annotation presence differs", |lhs, rhs| {
        type_eq(lhs, rhs, version)
    })
}

/// Structural equivalence of two Luau types, tolerating the formatter's
/// semantics-neutral rewrites (redundant parens, leading union/intersection
/// separators, and `,`/`;` table-field separators).
fn type_eq(left: &Type, right: &Type, version: LuaVersion) -> ControlFlow<AstDiff> {
    // As with expression parens in `expr_eq`, the formatter may add or drop
    // redundant type parens.
    let left_ty = unwrap_type_parens(left);
    let right_ty = unwrap_type_parens(right);
    match (left_ty, right_ty) {
        // Unreachable after `unwrap_type_parens`, but recursing is safer than
        // panicking.
        (Type::Parenthesized(inner), _) => type_eq(&inner.type_value, right_ty, version),
        (Type::Name { name: lhs, .. }, Type::Name { name: rhs, .. }) => {
            at("name", require(lhs == rhs, "type name differs"))
        }
        (Type::Name { name, .. }, Type::Named(named))
        | (Type::Named(named), Type::Name { name, .. }) => simple_named_type_eq(name, named),
        (Type::Named(lhs), Type::Named(rhs)) => named_type_eq(lhs, rhs, version),
        (Type::Typeof(lhs), Type::Typeof(rhs)) => {
            at("Typeof/expr", expr_eq(&lhs.expr, &rhs.expr, version))
        }
        (Type::Table(lhs), Type::Table(rhs)) => table_type_eq(lhs, rhs, version),
        (Type::Function(lhs), Type::Function(rhs)) => function_type_eq(lhs, rhs, version),
        (Type::Optional(lhs), Type::Optional(rhs)) => {
            type_eq(&lhs.type_value, &rhs.type_value, version)
        }
        // `leading_pipe` is trivia the formatter may add or remove.
        (Type::Union(lhs), Type::Union(rhs)) => {
            at("Union/types", punctuated_eq(&lhs.types, &rhs.types, version, type_eq))
        }
        // `leading_ampersand` is trivia the formatter may add or remove.
        (Type::Intersection(lhs), Type::Intersection(rhs)) => {
            at("Intersection/types", punctuated_eq(&lhs.types, &rhs.types, version, type_eq))
        }
        (Type::Pack(lhs), Type::Pack(rhs)) => {
            at("Pack/types", punctuated_eq(&lhs.types, &rhs.types, version, type_eq))
        }
        (Type::Singleton(lhs), Type::Singleton(rhs)) => {
            require(singleton_eq(lhs, rhs, version), "singleton type differs")
        }
        (Type::Variadic(lhs), Type::Variadic(rhs)) => {
            type_eq(&lhs.type_value, &rhs.type_value, version)
        }
        // The trailing `...` is fixed syntax; only the pack name varies.
        (Type::GenericPack(lhs), Type::GenericPack(rhs)) => token_text_eq(&lhs.name, &rhs.name),
        (Type::Error(_), Type::Error(_)) => Continue(()),
        (
            Type::Name { .. }
            | Type::Named(_)
            | Type::Typeof(_)
            | Type::Table(_)
            | Type::Function(_)
            | Type::Optional(_)
            | Type::Union(_)
            | Type::Intersection(_)
            | Type::Pack(_)
            | Type::Singleton(_)
            | Type::Variadic(_)
            | Type::GenericPack(_)
            | Type::Error(_),
            _,
        ) => differ(format!(
            "type kind differs: `{}` vs `{}`",
            type_kind(left_ty),
            type_kind(right_ty)
        )),
    }
}

/// Strip any nesting of redundant type parens down to the inner type.
fn unwrap_type_parens(ty: &Type) -> &Type {
    let mut current = ty;
    while let Type::Parenthesized(inner) = current {
        current = &inner.type_value;
    }
    current
}

fn named_type_eq(left: &NamedType, right: &NamedType, version: LuaVersion) -> ControlFlow<AstDiff> {
    presence_eq(
        left.prefix.as_ref(),
        right.prefix.as_ref(),
        "named type module presence differs",
        |lhs, rhs| at("prefix", token_text_eq(lhs, rhs)),
    )?;
    at("name", token_text_eq(&left.name, &right.name))?;
    at("generics", type_args_eq(left.generics.as_ref(), right.generics.as_ref(), version))
}

fn type_args_eq(
    left: Option<&TypeArgs>,
    right: Option<&TypeArgs>,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    presence_eq(left, right, "type argument list presence differs", |lhs, rhs| {
        punctuated_eq(&lhs.args, &rhs.args, version, type_eq)
    })
}

fn table_type_eq(left: &TableType, right: &TableType, version: LuaVersion) -> ControlFlow<AstDiff> {
    if left.fields.len() != right.fields.len() {
        return differ(format!(
            "table type field count differs: {} vs {}",
            left.fields.len(),
            right.fields.len()
        ));
    }
    // Field separators (`,` or `;`) are trivia; only the fields matter.
    for (index, (lhs, rhs)) in left.fields.iter().zip(right.fields.iter()).enumerate() {
        at_index("fields", index, type_field_eq(lhs, rhs, version))?;
    }
    Continue(())
}

fn type_field_eq(left: &TypeField, right: &TypeField, version: LuaVersion) -> ControlFlow<AstDiff> {
    match (left, right) {
        (
            TypeField::Named { access: left_access, name: left_name, value: left_value, .. },
            TypeField::Named { access: right_access, name: right_name, value: right_value, .. },
        ) => {
            at("access", access_modifier_eq(left_access.as_ref(), right_access.as_ref()))?;
            at("name", token_text_eq(left_name, right_name))?;
            at("value", type_eq(left_value, right_value, version))
        }
        (
            TypeField::Indexer { access: left_access, key: left_key, value: left_value, .. },
            TypeField::Indexer { access: right_access, key: right_key, value: right_value, .. },
        ) => {
            at("access", access_modifier_eq(left_access.as_ref(), right_access.as_ref()))?;
            at("key", type_eq(left_key, right_key, version))?;
            at("value", type_eq(left_value, right_value, version))
        }
        (
            TypeField::Array { value: left_value, .. },
            TypeField::Array { value: right_value, .. },
        ) => type_eq(left_value, right_value, version),
        (TypeField::Named { .. } | TypeField::Indexer { .. } | TypeField::Array { .. }, _) => {
            differ("table type field kind differs")
        }
    }
}

/// `read`/`write` modifiers are semantic, so presence and text have to match.
fn access_modifier_eq(left: Option<&Token>, right: Option<&Token>) -> ControlFlow<AstDiff> {
    presence_eq(left, right, "access modifier presence differs", token_text_eq)
}

fn function_type_eq(
    left: &FunctionType,
    right: &FunctionType,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    at("generics", generics_eq(left.generics.as_ref(), right.generics.as_ref(), version))?;
    at("params", punctuated_eq(&left.params, &right.params, version, function_type_param_eq))?;
    at("return_type", type_eq(&left.return_type, &right.return_type, version))
}

fn function_type_param_eq(
    left: &FunctionTypeParam,
    right: &FunctionTypeParam,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    // Function-type param names are documentation in Luau, but the formatter
    // never drops them, so compare presence and text strictly.
    presence_eq(
        left.name.as_ref(),
        right.name.as_ref(),
        "function type parameter name presence differs",
        |lhs, rhs| at("name", token_text_eq(lhs, rhs)),
    )?;
    type_eq(&left.type_value, &right.type_value, version)
}

/// Takes `Option<&_>` so the boxed (`FunctionBody`, `TypeDeclaration`) and
/// unboxed (`FunctionType`) generic lists share one comparison.
fn generics_eq(
    left: Option<&GenericTypeList>,
    right: Option<&GenericTypeList>,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    presence_eq(left, right, "generic parameter list presence differs", |lhs, rhs| {
        punctuated_eq(&lhs.params, &rhs.params, version, generic_param_eq)
    })
}

fn generic_param_eq(
    left: &GenericTypeParam,
    right: &GenericTypeParam,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    token_text_eq(&left.name, &right.name)?;
    require(left.is_pack == right.is_pack, "generic pack marker presence differs")?;
    presence_eq(
        left.default.as_ref(),
        right.default.as_ref(),
        "generic default presence differs",
        |lhs, rhs| at("default", type_eq(lhs, rhs, version)),
    )
}

/// A bare `Type::Name` against a `NamedType`, reporting at the paths
/// `named_type_eq` uses.
fn simple_named_type_eq(name: &str, named: &NamedType) -> ControlFlow<AstDiff> {
    if named.prefix.is_some() {
        return differ("named type module presence differs");
    }
    if !matches!(&named.name.kind, TokenKind::Identifier(other) if other == name) {
        return at("name", differ("type name differs"));
    }
    if named.generics.is_some() {
        return at("generics", differ("type argument list presence differs"));
    }
    Continue(())
}

const fn type_kind(ty: &Type) -> &'static str {
    match ty {
        Type::Name { .. } | Type::Named(_) => "Named",
        Type::Typeof(_) => "Typeof",
        Type::Table(_) => "Table",
        Type::Function(_) => "Function",
        Type::Optional(_) => "Optional",
        Type::Union(_) => "Union",
        Type::Intersection(_) => "Intersection",
        Type::Parenthesized(_) => "Parenthesized",
        Type::Pack(_) => "Pack",
        Type::Singleton(_) => "Singleton",
        Type::Variadic(_) => "Variadic",
        Type::GenericPack(_) => "GenericPack",
        Type::Error(_) => "Error",
    }
}

fn table_eq(
    left: &TableConstructor,
    right: &TableConstructor,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    if left.fields.len() != right.fields.len() {
        return differ(format!(
            "table field count differs: {} vs {}",
            left.fields.len(),
            right.fields.len()
        ));
    }
    for (index, (lhs, rhs)) in left.fields.iter().zip(right.fields.iter()).enumerate() {
        at_index("fields", index, field_eq(lhs, rhs, version))?;
    }
    Continue(())
}

fn field_eq(left: &Field, right: &Field, version: LuaVersion) -> ControlFlow<AstDiff> {
    match (left, right) {
        (
            Field::Named { name: left_name, value: left_value, .. },
            Field::Named { name: right_name, value: right_value, .. },
        ) => {
            at("name", token_text_eq(left_name, right_name))?;
            at("value", expr_eq(left_value, right_value, version))
        }
        (
            Field::Bracketed { key: left_key, value: left_value, .. },
            Field::Bracketed { key: right_key, value: right_value, .. },
        ) => {
            at("key", expr_eq(left_key, right_key, version))?;
            at("value", expr_eq(left_value, right_value, version))
        }
        (
            Field::Positional { value: left_value, .. },
            Field::Positional { value: right_value, .. },
        ) => expr_eq(left_value, right_value, version),
        (Field::Named { .. } | Field::Bracketed { .. } | Field::Positional { .. }, _) => {
            differ("field kind differs")
        }
    }
}

fn punctuated_eq<T>(
    left: &Punctuated<T>,
    right: &Punctuated<T>,
    version: LuaVersion,
    item_eq: fn(&T, &T, LuaVersion) -> ControlFlow<AstDiff>,
) -> ControlFlow<AstDiff> {
    let left_count = left.len();
    let right_count = right.len();
    if left_count != right_count {
        return differ(format!("punctuated length differs: {left_count} vs {right_count}"));
    }
    for (index, (lhs, rhs)) in left.iter().zip(right.iter()).enumerate() {
        at_index("", index, item_eq(lhs, rhs, version))?;
    }
    Continue(())
}

fn attributed_name_eq(
    left: &AttributedName,
    right: &AttributedName,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    token_text_eq(&left.name, &right.name)?;
    at(
        "type_annotation",
        type_annotation_eq(left.type_annotation.as_ref(), right.type_annotation.as_ref(), version),
    )?;
    presence_eq(
        left.attribute.as_deref(),
        right.attribute.as_deref(),
        "attribute presence differs",
        |lhs, rhs| at("attribute", token_text_eq(&lhs.name, &rhs.name)),
    )
}

fn token_text_eq(left: &Token, right: &Token) -> ControlFlow<AstDiff> {
    // `TokenKind::Identifier` carries the name, so comparing kinds compares
    // spelling.
    require(left.kind == right.kind, "token kind differs")
}

/// Compare two raw number texts, tolerating only the formatter's own
/// rewrites.
///
/// Those are lowercasing the base prefix and exponent marker, changing hex
/// digit case, and adding a zero before a bare leading dot (`.5` -> `0.5`).
/// A case-insensitive comparison after undoing the leading zero allows just
/// those. `0x10` and `16` still differ, since the formatter never changes
/// base.
fn number_raw_eq(left_raw: &str, right_raw: &str) -> bool {
    without_bare_zero(left_raw).eq_ignore_ascii_case(without_bare_zero(right_raw))
}

/// `raw` with the zero of a leading `0.` dropped.
fn without_bare_zero(raw: &str) -> &str {
    raw.strip_prefix('0').filter(|rest| rest.starts_with('.')).unwrap_or(raw)
}

/// Compare two raw string literals (quotes and escapes included) by the
/// bytes they decode to under `version`, so a quote-style swap (`'x'` ->
/// `"x"`, re-escaped as needed) still matches.
fn string_raw_eq(left_raw: &str, right_raw: &str, version: LuaVersion) -> bool {
    match (decode_string(left_raw, version), decode_string(right_raw, version)) {
        (Some(left_value), Some(right_value)) => left_value == right_value,
        // A literal the decoder doesn't model on either side falls back to
        // raw equality.
        _ => left_raw == right_raw,
    }
}

/// String singletons compare by decoded content, so a quote-style swap still
/// matches; the rest compare by kind.
fn singleton_eq(left: &Token, right: &Token, version: LuaVersion) -> bool {
    match (&left.kind, &right.kind) {
        (TokenKind::StringLiteral(left_raw), TokenKind::StringLiteral(right_raw)) => {
            string_raw_eq(left_raw, right_raw, version)
        }
        _ => left.kind == right.kind,
    }
}

const fn stmt_kind(stmt: &Statement) -> &'static str {
    match stmt {
        Statement::Assignment(_) => "Assignment",
        Statement::FunctionCall(_) => "FunctionCall",
        Statement::DoBlock(_) => "DoBlock",
        Statement::WhileLoop(_) => "WhileLoop",
        Statement::RepeatLoop(_) => "RepeatLoop",
        Statement::IfStatement(_) => "IfStatement",
        Statement::NumericFor(_) => "NumericFor",
        Statement::GenericFor(_) => "GenericFor",
        Statement::FunctionDecl(_) => "FunctionDecl",
        Statement::LocalFunction(_) => "LocalFunction",
        Statement::LocalAssignment(_) => "LocalAssignment",
        Statement::EmptyStatement(_) => "EmptyStatement",
        // Lua 5.2+
        Statement::Goto(_) => "Goto",
        // Lua 5.2+
        Statement::Label(_) => "Label",
        // Lua 5.5
        Statement::GlobalDeclaration(_) => "GlobalDeclaration",
        // Lua 5.5
        Statement::GlobalFunction(_) => "GlobalFunction",
        // Lua 5.5
        Statement::GlobalStar(_) => "GlobalStar",
        // Lua 5.2+
        Statement::Break(_) => "Break",
        // Luau
        Statement::CompoundAssignment(_) => "CompoundAssignment",
        // Luau
        Statement::TypeDeclaration(_) => "TypeDeclaration",
        Statement::Error(_) => "Error",
    }
}

const fn expr_kind(expr: &Expression) -> &'static str {
    match expr {
        Expression::Nil(_) => "Nil",
        Expression::False(_) => "False",
        Expression::True(_) => "True",
        Expression::Number(_) => "Number",
        // Luau
        Expression::Integer(_) => "Integer",
        Expression::StringLiteral(_) => "StringLiteral",
        Expression::VarArg(_) => "VarArg",
        Expression::FunctionDef(_) => "FunctionDef",
        Expression::Var(_) => "Var",
        Expression::FunctionCall(_) => "FunctionCall",
        Expression::Parenthesized(_) => "Parenthesized",
        Expression::TableConstructor(_) => "TableConstructor",
        Expression::BinaryOp(_) => "BinaryOp",
        Expression::UnaryOp(_) => "UnaryOp",
        // Luau
        Expression::IfExpression(_) => "IfExpression",
        // Luau
        Expression::InterpolatedString(_) => "InterpolatedString",
        // Luau
        Expression::TypeCast(_) => "TypeCast",
        // Luau
        Expression::TypeInstantiation(_) => "TypeInstantiation",
        Expression::Error(_) => "Error",
    }
}

#[cfg(test)]
mod tests {
    use luck_ast::Synth;

    use super::*;

    /// Where `left` and `right`, parsed as `version`, first diverge.
    fn divergence(version: LuaVersion, left: &str, right: &str) -> Option<AstDiff> {
        let [left_block, right_block] = [left, right].map(|source| {
            let parsed = luck_parser::parse(source, version);
            assert!(parsed.errors.is_empty(), "parse error: {:?}", parsed.errors);
            parsed.block
        });
        blocks_equivalent(&left_block, &right_block, version).err()
    }

    #[test]
    fn equivalent_when_formatting_differs() {
        let diff =
            divergence(LuaVersion::Lua54, "local x = 1\nlocal y = 2", "local x=1\nlocal y=2");
        assert_eq!(diff, None);
    }

    #[test]
    fn paren_strip_is_equivalent() {
        let diff = divergence(LuaVersion::Lua54, "local x = (1 + 2)", "local x = 1 + 2");
        assert_eq!(diff, None);
    }

    #[test]
    fn call_paren_normalization_is_equivalent() {
        let diff = divergence(LuaVersion::Lua54, "print(\"hi\")", "print\"hi\"");
        assert_eq!(diff, None);
    }

    #[test]
    fn detects_renamed_identifier() {
        // `TokenKind::Identifier` carries the name, so a rename changes the
        // token kind.
        let diff = divergence(LuaVersion::Lua54, "local x = 1", "local y = 1")
            .expect("rename should diverge");
        assert!(diff.reason.contains("kind"), "got: {}", diff.reason);
    }

    #[test]
    fn detects_missing_stmt() {
        let diff = divergence(LuaVersion::Lua54, "local x = 1\nlocal y = 2", "local x = 1")
            .expect("should diverge");
        assert!(diff.reason.contains("count"));
    }

    #[test]
    fn detects_kind_change() {
        let diff = divergence(LuaVersion::Lua54, "local x = 1", "x = 1").expect("should diverge");
        assert!(diff.reason.contains("kind"));
    }

    #[test]
    fn identical_type_aliases_are_equivalent() {
        let alias = "type Pair = { first: number, second: string }";
        assert_eq!(divergence(LuaVersion::Luau, alias, alias), None);
    }

    #[test]
    fn compact_and_general_type_names_are_equivalent() {
        let synth = Synth::new();
        let compact = synth.ty_named("number");
        let general = Type::Named(Box::new(NamedType {
            span: luck_token::Span::new(0, 0),
            prefix: None,
            name: synth.identifier("number"),
            generics: None,
        }));
        let version = LuaVersion::Luau;
        assert!(type_eq(&compact, &general, version).is_continue());
        assert!(type_eq(&general, &compact, version).is_continue());
        for different in [
            synth.ty_named("string"),
            synth.ty_qualified("module", "number"),
            synth.ty_generic("number", vec![synth.ty_named("T")]),
        ] {
            assert!(type_eq(&compact, &different, version).is_break());
            assert!(type_eq(&different, &compact, version).is_break());
        }
    }

    #[test]
    fn string_singleton_quote_style_is_equivalent() {
        // Singletons compare by decoded content, so a quote swap still
        // matches.
        let diff = divergence(LuaVersion::Luau, "type State = \"on\"", "type State = 'on'");
        assert_eq!(diff, None);
    }

    #[test]
    fn redundant_type_parens_are_equivalent() {
        let diff = divergence(LuaVersion::Luau, "local x: (number) = 1", "local x: number = 1");
        assert_eq!(diff, None);
    }

    #[test]
    fn leading_union_pipe_is_equivalent() {
        // A leading `|` is trivia the formatter may add or remove.
        let diff = divergence(
            LuaVersion::Luau,
            "type Value = | number | string",
            "type Value = number | string",
        );
        assert_eq!(diff, None);
    }

    #[test]
    fn detects_differing_annotation_type() {
        let diff = divergence(LuaVersion::Luau, "local x: number = 1", "local x: string = 1")
            .expect("differing types should diverge");
        assert!(diff.path.contains("type"), "got path: {}", diff.path);
    }

    #[test]
    fn detects_dropped_annotation() {
        // A dropped annotation is data loss, never a tolerated rewrite.
        let diff = divergence(LuaVersion::Luau, "local x: number = 1", "local x = 1")
            .expect("dropped annotation should diverge");
        assert!(diff.reason.contains("presence differs"), "got: {}", diff.reason);
    }
}
