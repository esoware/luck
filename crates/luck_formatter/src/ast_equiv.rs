//! Structural AST equivalence used by `--verify` and `format_and_verify`.
//!
//! The check ignores spans, comments, and trivia - it only compares the
//! shape and identifier text of the syntax tree. Two ASTs that print to
//! semantically identical Lua should be considered equivalent.

use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Literal, Var};
use luck_ast::shared::{Block, Field, FunctionBody, Parameter, Punctuated};
use luck_ast::stmt::{LastStatement, Statement, TypeDeclarationValue};
use luck_ast::types::{
    FunctionType, FunctionTypeParam, GenericTypeList, GenericTypeParam, NamedType, TableType, Type,
    TypeArgs, TypeField,
};
use luck_token::Token;

/// First point of divergence between two ASTs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstDiff {
    /// Slash-separated path into the tree, e.g. `block/stmt[3]/FunctionDecl/body`.
    pub path: String,
    pub reason: String,
}

impl AstDiff {
    fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            reason: reason.into(),
        }
    }
}

fn child(path: &str, segment: &str) -> String {
    if path.is_empty() {
        segment.to_string()
    } else {
        format!("{path}/{segment}")
    }
}

/// Top-level entry: compare two blocks structurally.
pub fn blocks_equiv(left: &Block, right: &Block) -> Result<(), AstDiff> {
    block_eq(left, right, "block")
}

fn block_eq(left: &Block, right: &Block, path: &str) -> Result<(), AstDiff> {
    // The formatter intentionally drops `;` empty statements and inserts a
    // disambiguating `;` before `(`-starting statements - both are
    // semantics-neutral, so compare with EmptyStatement filtered out.
    let left_stmts: Vec<&Statement> = left
        .stmts
        .iter()
        .filter(|stmt| !matches!(stmt, Statement::EmptyStatement(_)))
        .collect();
    let right_stmts: Vec<&Statement> = right
        .stmts
        .iter()
        .filter(|stmt| !matches!(stmt, Statement::EmptyStatement(_)))
        .collect();

    // Statements may legitimately reorder (sort_requires); we still require
    // identical length and pointwise structural equality after the rewrite.
    if left_stmts.len() != right_stmts.len() {
        return Err(AstDiff::new(
            path,
            format!(
                "statement count differs: {} vs {}",
                left_stmts.len(),
                right_stmts.len()
            ),
        ));
    }
    for (idx, (l, r)) in left_stmts.iter().zip(right_stmts.iter()).enumerate() {
        stmt_eq(l, r, &child(path, &format!("stmt[{idx}]")))?;
    }
    match (&left.last_stmt, &right.last_stmt) {
        (None, None) => Ok(()),
        (Some(l), Some(r)) => last_stmt_eq(l, r, &child(path, "last_stmt")),
        (Some(_), None) | (None, Some(_)) => Err(AstDiff::new(
            path,
            "last statement presence differs".to_string(),
        )),
    }
}

fn stmt_eq(left: &Statement, right: &Statement, path: &str) -> Result<(), AstDiff> {
    match (left, right) {
        (Statement::Assignment(l), Statement::Assignment(r)) => {
            punctuated_eq(
                &l.targets,
                &r.targets,
                &child(path, "Assignment/targets"),
                var_eq,
            )?;
            punctuated_eq(
                &l.values,
                &r.values,
                &child(path, "Assignment/values"),
                expr_eq,
            )
        }
        (Statement::FunctionCall(l), Statement::FunctionCall(r)) => {
            function_call_eq(&l.call, &r.call, &child(path, "FunctionCall"))
        }
        (Statement::DoBlock(l), Statement::DoBlock(r)) => {
            block_eq(&l.block, &r.block, &child(path, "DoBlock"))
        }
        (Statement::WhileLoop(l), Statement::WhileLoop(r)) => {
            expr_eq(&l.condition, &r.condition, &child(path, "While/cond"))?;
            block_eq(&l.block, &r.block, &child(path, "While/body"))
        }
        (Statement::RepeatLoop(l), Statement::RepeatLoop(r)) => {
            block_eq(&l.block, &r.block, &child(path, "Repeat/body"))?;
            expr_eq(&l.condition, &r.condition, &child(path, "Repeat/until"))
        }
        (Statement::IfStatement(l), Statement::IfStatement(r)) => {
            expr_eq(&l.condition, &r.condition, &child(path, "If/cond"))?;
            block_eq(&l.block, &r.block, &child(path, "If/then"))?;
            if l.elseif_clauses.len() != r.elseif_clauses.len() {
                return Err(AstDiff::new(path, "elseif count differs"));
            }
            for (i, (a, b)) in l.elseif_clauses.iter().zip(&r.elseif_clauses).enumerate() {
                let pp = child(path, &format!("elseif[{i}]"));
                expr_eq(&a.condition, &b.condition, &child(&pp, "cond"))?;
                block_eq(&a.block, &b.block, &child(&pp, "body"))?;
            }
            match (&l.else_clause, &r.else_clause) {
                (None, None) => Ok(()),
                (Some(a), Some(b)) => block_eq(&a.block, &b.block, &child(path, "else")),
                _ => Err(AstDiff::new(path, "else presence differs")),
            }
        }
        (Statement::NumericFor(l), Statement::NumericFor(r)) => {
            token_text_eq(&l.name, &r.name, &child(path, "NumericFor/name"))?;
            type_annotation_eq(
                &l.type_annotation,
                &r.type_annotation,
                &child(path, "NumericFor/type"),
            )?;
            expr_eq(&l.start, &r.start, &child(path, "NumericFor/start"))?;
            expr_eq(&l.limit, &r.limit, &child(path, "NumericFor/limit"))?;
            match (&l.step, &r.step) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    expr_eq(a, b, &child(path, "NumericFor/step"))?;
                }
                _ => return Err(AstDiff::new(path, "NumericFor step presence differs")),
            }
            block_eq(&l.block, &r.block, &child(path, "NumericFor/body"))
        }
        (Statement::GenericFor(l), Statement::GenericFor(r)) => {
            punctuated_eq(
                &l.names,
                &r.names,
                &child(path, "GenericFor/names"),
                param_eq,
            )?;
            punctuated_eq(
                &l.exprs,
                &r.exprs,
                &child(path, "GenericFor/exprs"),
                expr_eq,
            )?;
            block_eq(&l.block, &r.block, &child(path, "GenericFor/body"))
        }
        (Statement::FunctionDecl(l), Statement::FunctionDecl(r)) => {
            // Dotted name pieces must match
            if l.name.names.len() != r.name.names.len() {
                return Err(AstDiff::new(path, "FunctionDecl name length differs"));
            }
            for (i, (a, b)) in l.name.names.iter().zip(&r.name.names).enumerate() {
                token_text_eq(a, b, &child(path, &format!("FunctionDecl/name[{i}]")))?;
            }
            match (&l.name.method, &r.name.method) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    token_text_eq(a, b, &child(path, "FunctionDecl/method"))?;
                }
                _ => return Err(AstDiff::new(path, "FunctionDecl method presence differs")),
            }
            function_attributes_eq(&l.attributes, &r.attributes, &child(path, "FunctionDecl"))?;
            function_body_eq(&l.body, &r.body, &child(path, "FunctionDecl/body"))
        }
        (Statement::LocalFunction(l), Statement::LocalFunction(r)) => {
            token_text_eq(&l.name, &r.name, &child(path, "LocalFunction/name"))?;
            if l.is_const != r.is_const {
                return Err(AstDiff::new(path, "LocalFunction const-ness differs"));
            }
            function_attributes_eq(&l.attributes, &r.attributes, &child(path, "LocalFunction"))?;
            function_body_eq(&l.body, &r.body, &child(path, "LocalFunction/body"))
        }
        (Statement::GlobalFunction(l), Statement::GlobalFunction(r)) => {
            token_text_eq(&l.name, &r.name, &child(path, "GlobalFunction/name"))?;
            function_body_eq(&l.body, &r.body, &child(path, "GlobalFunction/body"))
        }
        (Statement::LocalAssignment(l), Statement::LocalAssignment(r)) => {
            if l.is_const != r.is_const {
                return Err(AstDiff::new(path, "LocalAssignment const-ness differs"));
            }
            punctuated_eq(
                &l.names,
                &r.names,
                &child(path, "LocalAssignment/names"),
                attributed_name_eq,
            )?;
            match (&l.exprs, &r.exprs) {
                (None, None) => Ok(()),
                (Some(a), Some(b)) => {
                    punctuated_eq(a, b, &child(path, "LocalAssignment/exprs"), expr_eq)
                }
                _ => Err(AstDiff::new(path, "LocalAssignment exprs presence differs")),
            }
        }
        (Statement::EmptyStatement(_), Statement::EmptyStatement(_)) => Ok(()),
        (Statement::Goto(l), Statement::Goto(r)) => {
            token_text_eq(&l.name, &r.name, &child(path, "Goto/name"))
        }
        (Statement::Label(l), Statement::Label(r)) => {
            token_text_eq(&l.name, &r.name, &child(path, "Label/name"))
        }
        (Statement::GlobalDeclaration(l), Statement::GlobalDeclaration(r)) => {
            punctuated_eq(
                &l.names,
                &r.names,
                &child(path, "GlobalDeclaration/names"),
                attributed_name_eq,
            )?;
            match (&l.exprs, &r.exprs) {
                (None, None) => Ok(()),
                (Some(a), Some(b)) => {
                    punctuated_eq(a, b, &child(path, "GlobalDeclaration/exprs"), expr_eq)
                }
                _ => Err(AstDiff::new(
                    path,
                    "GlobalDeclaration exprs presence differs",
                )),
            }
        }
        (Statement::GlobalStar(_), Statement::GlobalStar(_)) => Ok(()),
        (Statement::Break(_), Statement::Break(_)) => Ok(()),
        (Statement::CompoundAssignment(l), Statement::CompoundAssignment(r)) => {
            var_eq(&l.var, &r.var, &child(path, "CompoundAssignment/var"))?;
            expr_eq(&l.expr, &r.expr, &child(path, "CompoundAssignment/expr"))
        }
        (Statement::TypeDeclaration(l), Statement::TypeDeclaration(r)) => {
            token_text_eq(&l.name, &r.name, &child(path, "TypeDeclaration/name"))?;
            generics_eq(
                l.generics.as_deref(),
                r.generics.as_deref(),
                &child(path, "TypeDeclaration/generics"),
            )?;
            match (&l.type_value, &r.type_value) {
                (TypeDeclarationValue::Alias(a), TypeDeclarationValue::Alias(b)) => {
                    types_equiv(a, b, &child(path, "TypeDeclaration/alias"))
                }
                (TypeDeclarationValue::TypeFunction(a), TypeDeclarationValue::TypeFunction(b)) => {
                    function_body_eq(a, b, &child(path, "TypeDeclaration/typefn"))
                }
                _ => Err(AstDiff::new(path, "type declaration value kind differs")),
            }
        }
        (Statement::Error(_), Statement::Error(_)) => Ok(()),
        _ => Err(AstDiff::new(
            path,
            format!(
                "statement kind differs: {} vs {}",
                stmt_kind(left),
                stmt_kind(right)
            ),
        )),
    }
}

fn last_stmt_eq(left: &LastStatement, right: &LastStatement, path: &str) -> Result<(), AstDiff> {
    match (left, right) {
        (LastStatement::Return(l), LastStatement::Return(r)) => {
            punctuated_eq(&l.exprs, &r.exprs, &child(path, "Return/exprs"), expr_eq)
        }
        (LastStatement::Break(_), LastStatement::Break(_)) => Ok(()),
        (LastStatement::Continue(_), LastStatement::Continue(_)) => Ok(()),
        (LastStatement::Error(_), LastStatement::Error(_)) => Ok(()),
        _ => Err(AstDiff::new(path, "last statement kind differs")),
    }
}

fn expr_eq(left: &Expression, right: &Expression, path: &str) -> Result<(), AstDiff> {
    match (left, right) {
        (Expression::Nil(_), Expression::Nil(_))
        | (Expression::False(_), Expression::False(_))
        | (Expression::True(_), Expression::True(_))
        | (Expression::VarArg(_), Expression::VarArg(_)) => Ok(()),
        (Expression::StringLiteral(a), Expression::StringLiteral(b)) => {
            // String literals must keep their textual contents; the formatter
            // is allowed to swap quote style, so compare unescaped contents.
            if string_raw_eq(&a.text, &b.text) {
                Ok(())
            } else {
                Err(AstDiff::new(path, "literal contents differ"))
            }
        }
        (Expression::Number(a), Expression::Number(b)) => {
            if number_raw_eq(&a.text, &b.text) {
                Ok(())
            } else {
                Err(AstDiff::new(path, "number literals differ"))
            }
        }
        (Expression::Var(a), Expression::Var(b)) => var_eq(a, b, path),
        (Expression::BinaryOp(a), Expression::BinaryOp(b)) => {
            if a.op != b.op {
                return Err(AstDiff::new(path, "binary op differs"));
            }
            expr_eq(&a.left, &b.left, &child(path, "lhs"))?;
            expr_eq(&a.right, &b.right, &child(path, "rhs"))
        }
        (Expression::UnaryOp(a), Expression::UnaryOp(b)) => {
            if a.op != b.op {
                return Err(AstDiff::new(path, "unary op differs"));
            }
            expr_eq(&a.operand, &b.operand, &child(path, "operand"))
        }
        (Expression::Parenthesized(a), Expression::Parenthesized(b)) => {
            expr_eq(&a.expr, &b.expr, &child(path, "parens"))
        }
        // The formatter may strip or add a single layer of parens; treat
        // `(x)` and `x` as structurally equivalent for verification.
        (Expression::Parenthesized(a), other) => expr_eq(&a.expr, other, path),
        (other, Expression::Parenthesized(b)) => expr_eq(other, &b.expr, path),
        (Expression::FunctionCall(a), Expression::FunctionCall(b)) => {
            function_call_eq(a, b, &child(path, "Call"))
        }
        (Expression::FunctionDef(a), Expression::FunctionDef(b)) => {
            function_attributes_eq(&a.attributes, &b.attributes, &child(path, "FunctionDef"))?;
            function_body_eq(&a.body, &b.body, &child(path, "FunctionDef"))
        }
        (Expression::TableConstructor(a), Expression::TableConstructor(b)) => {
            table_eq(a, b, &child(path, "Table"))
        }
        (Expression::IfExpression(a), Expression::IfExpression(b)) => {
            expr_eq(&a.condition, &b.condition, &child(path, "ifexpr/cond"))?;
            expr_eq(&a.then_expr, &b.then_expr, &child(path, "ifexpr/then"))?;
            if a.elseif_clauses.len() != b.elseif_clauses.len() {
                return Err(AstDiff::new(path, "ifexpr elseif count differs"));
            }
            for (i, (l, r)) in a.elseif_clauses.iter().zip(&b.elseif_clauses).enumerate() {
                let pp = child(path, &format!("ifexpr/elseif[{i}]"));
                expr_eq(&l.condition, &r.condition, &child(&pp, "cond"))?;
                expr_eq(&l.expr, &r.expr, &child(&pp, "expr"))?;
            }
            expr_eq(&a.else_expr, &b.else_expr, &child(path, "ifexpr/else"))
        }
        (Expression::InterpolatedString(_), Expression::InterpolatedString(_)) => {
            // Treated opaquely; the formatter copies the original span text.
            Ok(())
        }
        (Expression::TypeCast(a), Expression::TypeCast(b)) => {
            expr_eq(&a.expr, &b.expr, &child(path, "TypeCast/expr"))?;
            types_equiv(
                &a.type_annotation,
                &b.type_annotation,
                &child(path, "TypeCast/type"),
            )
        }
        (Expression::Error(_), Expression::Error(_)) => Ok(()),
        _ => Err(AstDiff::new(
            path,
            format!(
                "expression kind differs: {} vs {}",
                expr_kind(left),
                expr_kind(right)
            ),
        )),
    }
}

fn var_eq(left: &Var, right: &Var, path: &str) -> Result<(), AstDiff> {
    match (left, right) {
        (Var::Name(a), Var::Name(b)) => token_text_eq(a, b, path),
        (Var::FieldAccess(a), Var::FieldAccess(b)) => {
            expr_eq(&a.prefix, &b.prefix, &child(path, "prefix"))?;
            token_text_eq(&a.name, &b.name, &child(path, "name"))
        }
        (Var::Index(a), Var::Index(b)) => {
            expr_eq(&a.prefix, &b.prefix, &child(path, "prefix"))?;
            expr_eq(&a.index, &b.index, &child(path, "index"))
        }
        _ => Err(AstDiff::new(path, "var kind differs")),
    }
}

fn function_call_eq(left: &FunctionCall, right: &FunctionCall, path: &str) -> Result<(), AstDiff> {
    expr_eq(&left.callee, &right.callee, &child(path, "callee"))?;
    match (&left.method, &right.method) {
        (None, None) => {}
        (Some(a), Some(b)) => token_text_eq(a, b, &child(path, "method"))?,
        _ => return Err(AstDiff::new(path, "method presence differs")),
    }
    args_eq(&left.args, &right.args, &child(path, "args"))
}

fn args_eq(left: &FunctionArgs, right: &FunctionArgs, path: &str) -> Result<(), AstDiff> {
    let left_args = normalize_args(left);
    let right_args = normalize_args(right);
    match (left_args, right_args) {
        (NormalizedArgs::List(a), NormalizedArgs::List(b)) => punctuated_eq(a, b, path, expr_eq),
        (NormalizedArgs::Table(a), NormalizedArgs::Table(b)) => table_eq(a, b, path),
        (NormalizedArgs::String(a), NormalizedArgs::String(b)) => {
            if string_raw_eq(&a.text, &b.text) {
                Ok(())
            } else {
                Err(AstDiff::new(path, "call string arg differs"))
            }
        }
        // The formatter is allowed to convert `f"x"` <-> `f("x")` and `f{1}` <-> `f({1})`.
        (NormalizedArgs::String(s), NormalizedArgs::List(list))
        | (NormalizedArgs::List(list), NormalizedArgs::String(s)) => {
            if list.len() == 1
                && let Some(Expression::StringLiteral(other)) = list.first()
                && string_raw_eq(&s.text, &other.text)
            {
                return Ok(());
            }
            Err(AstDiff::new(path, "call arg shape differs"))
        }
        (NormalizedArgs::Table(table), NormalizedArgs::List(list))
        | (NormalizedArgs::List(list), NormalizedArgs::Table(table)) => {
            if list.len() == 1
                && let Some(Expression::TableConstructor(other)) = list.first()
            {
                return table_eq(table, other, path);
            }
            Err(AstDiff::new(path, "call arg shape differs"))
        }
        _ => Err(AstDiff::new(path, "call arg shape differs")),
    }
}

enum NormalizedArgs<'a> {
    List(&'a Punctuated<Expression>),
    Table(&'a luck_ast::expr::TableConstructor),
    String(&'a Literal),
}

fn normalize_args(args: &FunctionArgs) -> NormalizedArgs<'_> {
    match args {
        FunctionArgs::Parenthesized { args, .. } => NormalizedArgs::List(args),
        FunctionArgs::TableConstructor(table) => NormalizedArgs::Table(table),
        FunctionArgs::StringLiteral(literal) => NormalizedArgs::String(literal),
    }
}

/// Attribute lists compare by name and argument values; dropping or
/// reordering `@native`/`@[deprecated(...)]` changes runtime behavior.
fn function_attributes_eq(
    left: &[luck_ast::stmt::FunctionAttribute],
    right: &[luck_ast::stmt::FunctionAttribute],
    path: &str,
) -> Result<(), AstDiff> {
    if left.len() != right.len() {
        return Err(AstDiff::new(path, "attribute count differs"));
    }
    for (i, (l, r)) in left.iter().zip(right).enumerate() {
        token_text_eq(&l.name, &r.name, &child(path, &format!("attr[{i}]")))?;
        match (&l.args, &r.args) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                punctuated_eq(a, b, &child(path, &format!("attr[{i}]/args")), expr_eq)?;
            }
            _ => return Err(AstDiff::new(path, "attribute args presence differs")),
        }
    }
    Ok(())
}

fn function_body_eq(left: &FunctionBody, right: &FunctionBody, path: &str) -> Result<(), AstDiff> {
    generics_eq(
        left.generics.as_deref(),
        right.generics.as_deref(),
        &child(path, "generics"),
    )?;
    punctuated_eq(
        &left.params,
        &right.params,
        &child(path, "params"),
        param_eq,
    )?;
    match (&left.vararg, &right.vararg) {
        (None, None) => {}
        (Some(la), Some(ra)) => type_annotation_eq(
            &la.type_annotation,
            &ra.type_annotation,
            &child(path, "vararg/type"),
        )?,
        _ => return Err(AstDiff::new(path, "vararg presence differs")),
    }
    type_annotation_eq(
        &left.return_type,
        &right.return_type,
        &child(path, "return"),
    )?;
    block_eq(&left.block, &right.block, &child(path, "body"))
}

fn param_eq(left: &Parameter, right: &Parameter, path: &str) -> Result<(), AstDiff> {
    token_text_eq(&left.name, &right.name, path)?;
    type_annotation_eq(
        &left.type_annotation,
        &right.type_annotation,
        &child(path, "type"),
    )
}

/// Compare an optional `: Type` annotation. Presence must agree on both
/// sides - a dropped annotation is data loss, not a formatter tolerance.
fn type_annotation_eq(
    left: &Option<Type>,
    right: &Option<Type>,
    path: &str,
) -> Result<(), AstDiff> {
    match (left, right) {
        (None, None) => Ok(()),
        (Some(a), Some(b)) => types_equiv(a, b, path),
        _ => Err(AstDiff::new(path, "type annotation presence differs")),
    }
}

/// Structural equivalence of two Luau types, tolerating the formatter's
/// semantics-neutral rewrites (redundant parens, leading union/intersection
/// separators, and `,`/`;` table-field separators).
fn types_equiv(left: &Type, right: &Type, path: &str) -> Result<(), AstDiff> {
    // `(T)` is equivalent to `T`: the formatter may add or drop redundant type parens, so
    // strip them from both sides before matching, mirroring the expression
    // paren rule in `expr_eq`.
    let left = unwrap_type_parens(left);
    let right = unwrap_type_parens(right);
    match left {
        // Unreachable after `unwrap_type_parens`, but keeps the match
        // exhaustive; recurse rather than panic.
        Type::Parenthesized(inner) => types_equiv(&inner.type_value, right, path),
        Type::Named(a) => match right {
            Type::Named(b) => named_type_eq(a, b, path),
            _ => Err(type_kind_diff(left, right, path)),
        },
        Type::Typeof(a) => match right {
            Type::Typeof(b) => expr_eq(&a.expr, &b.expr, &child(path, "typeof")),
            _ => Err(type_kind_diff(left, right, path)),
        },
        Type::Table(a) => match right {
            Type::Table(b) => table_type_eq(a, b, path),
            _ => Err(type_kind_diff(left, right, path)),
        },
        Type::Function(a) => match right {
            Type::Function(b) => function_type_eq(a, b, path),
            _ => Err(type_kind_diff(left, right, path)),
        },
        Type::Optional(a) => match right {
            Type::Optional(b) => types_equiv(&a.type_value, &b.type_value, path),
            _ => Err(type_kind_diff(left, right, path)),
        },
        Type::Union(a) => match right {
            // `leading_pipe` is trivia the formatter may add or remove.
            Type::Union(b) => punctuated_eq(&a.types, &b.types, &child(path, "union"), types_equiv),
            _ => Err(type_kind_diff(left, right, path)),
        },
        Type::Intersection(a) => match right {
            // `leading_ampersand` is trivia the formatter may add or remove.
            Type::Intersection(b) => punctuated_eq(
                &a.types,
                &b.types,
                &child(path, "intersection"),
                types_equiv,
            ),
            _ => Err(type_kind_diff(left, right, path)),
        },
        Type::Pack(a) => match right {
            Type::Pack(b) => punctuated_eq(&a.types, &b.types, &child(path, "pack"), types_equiv),
            _ => Err(type_kind_diff(left, right, path)),
        },
        Type::Singleton(a) => match right {
            // Singleton tokens (string/`true`/`false`/`nil`) carry their
            // decoded value in the kind, so string singletons compare by
            // content and are quote-style-agnostic.
            Type::Singleton(b) if singleton_eq(a, b) => Ok(()),
            Type::Singleton(_) => Err(AstDiff::new(path, "singleton type differs")),
            _ => Err(type_kind_diff(left, right, path)),
        },
        Type::Variadic(a) => match right {
            Type::Variadic(b) => types_equiv(&a.type_value, &b.type_value, path),
            _ => Err(type_kind_diff(left, right, path)),
        },
        Type::GenericPack(a) => match right {
            // The trailing `...` is fixed syntax; only the pack name varies.
            Type::GenericPack(b) => token_text_eq(&a.name, &b.name, path),
            _ => Err(type_kind_diff(left, right, path)),
        },
        Type::Error(_) => match right {
            Type::Error(_) => Ok(()),
            _ => Err(type_kind_diff(left, right, path)),
        },
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

fn named_type_eq(left: &NamedType, right: &NamedType, path: &str) -> Result<(), AstDiff> {
    match (&left.prefix, &right.prefix) {
        (None, None) => {}
        (Some(lm), Some(rm)) => token_text_eq(lm, rm, &child(path, "module"))?,
        _ => return Err(AstDiff::new(path, "named type module presence differs")),
    }
    token_text_eq(&left.name, &right.name, &child(path, "name"))?;
    type_args_eq(&left.generics, &right.generics, &child(path, "generics"))
}

fn type_args_eq(
    left: &Option<TypeArgs>,
    right: &Option<TypeArgs>,
    path: &str,
) -> Result<(), AstDiff> {
    match (left, right) {
        (None, None) => Ok(()),
        (Some(a), Some(b)) => punctuated_eq(&a.args, &b.args, path, types_equiv),
        _ => Err(AstDiff::new(path, "type argument list presence differs")),
    }
}

fn table_type_eq(left: &TableType, right: &TableType, path: &str) -> Result<(), AstDiff> {
    if left.fields.len() != right.fields.len() {
        return Err(AstDiff::new(
            path,
            format!(
                "table type field count differs: {} vs {}",
                left.fields.len(),
                right.fields.len()
            ),
        ));
    }
    // The following separator (`,` vs `;`) is trivia; only the field matters.
    for (idx, (lf, rf)) in left.fields.iter().zip(right.fields.iter()).enumerate() {
        type_field_eq(lf, rf, &child(path, &format!("field[{idx}]")))?;
    }
    Ok(())
}

fn type_field_eq(left: &TypeField, right: &TypeField, path: &str) -> Result<(), AstDiff> {
    match (left, right) {
        (
            TypeField::Named {
                access: la,
                name: ln,
                value: lv,
                ..
            },
            TypeField::Named {
                access: ra,
                name: rn,
                value: rv,
                ..
            },
        ) => {
            access_modifier_eq(la, ra, &child(path, "access"))?;
            token_text_eq(ln, rn, &child(path, "name"))?;
            types_equiv(lv, rv, &child(path, "value"))
        }
        (
            TypeField::Indexer {
                access: la,
                key: lk,
                value: lv,
                ..
            },
            TypeField::Indexer {
                access: ra,
                key: rk,
                value: rv,
                ..
            },
        ) => {
            access_modifier_eq(la, ra, &child(path, "access"))?;
            types_equiv(lk, rk, &child(path, "key"))?;
            types_equiv(lv, rv, &child(path, "value"))
        }
        (TypeField::Array { value: lv, .. }, TypeField::Array { value: rv, .. }) => {
            types_equiv(lv, rv, path)
        }
        _ => Err(AstDiff::new(path, "table type field kind differs")),
    }
}

/// `read`/`write` access modifiers are semantic - presence and text matter.
fn access_modifier_eq(
    left: &Option<Token>,
    right: &Option<Token>,
    path: &str,
) -> Result<(), AstDiff> {
    match (left, right) {
        (None, None) => Ok(()),
        (Some(l), Some(r)) => token_text_eq(l, r, path),
        _ => Err(AstDiff::new(path, "access modifier presence differs")),
    }
}

fn function_type_eq(left: &FunctionType, right: &FunctionType, path: &str) -> Result<(), AstDiff> {
    generics_eq(
        left.generics.as_ref(),
        right.generics.as_ref(),
        &child(path, "generics"),
    )?;
    punctuated_eq(
        &left.params,
        &right.params,
        &child(path, "params"),
        function_type_param_eq,
    )?;
    types_equiv(
        &left.return_type,
        &right.return_type,
        &child(path, "return"),
    )
}

fn function_type_param_eq(
    left: &FunctionTypeParam,
    right: &FunctionTypeParam,
    path: &str,
) -> Result<(), AstDiff> {
    // Function-type param names are documentation in Luau, but the formatter
    // never drops them, so compare presence and text strictly.
    match (&left.name, &right.name) {
        (None, None) => {}
        (Some(l), Some(r)) => token_text_eq(l, r, &child(path, "name"))?,
        _ => {
            return Err(AstDiff::new(
                path,
                "function type parameter name presence differs",
            ));
        }
    }
    types_equiv(&left.type_value, &right.type_value, path)
}

/// Compare optional generic parameter lists. Accepts `Option<&T>` so both the
/// boxed (`FunctionBody`, `TypeDeclaration`) and unboxed (`FunctionType`)
/// storage forms funnel through one comparison.
fn generics_eq(
    left: Option<&GenericTypeList>,
    right: Option<&GenericTypeList>,
    path: &str,
) -> Result<(), AstDiff> {
    match (left, right) {
        (None, None) => Ok(()),
        (Some(a), Some(b)) => punctuated_eq(&a.params, &b.params, path, generic_param_eq),
        _ => Err(AstDiff::new(
            path,
            "generic parameter list presence differs",
        )),
    }
}

fn generic_param_eq(
    left: &GenericTypeParam,
    right: &GenericTypeParam,
    path: &str,
) -> Result<(), AstDiff> {
    token_text_eq(&left.name, &right.name, path)?;
    if left.is_pack != right.is_pack {
        return Err(AstDiff::new(path, "generic pack marker presence differs"));
    }
    match (&left.default, &right.default) {
        (None, None) => Ok(()),
        (Some(a), Some(b)) => types_equiv(a, b, &child(path, "default")),
        _ => Err(AstDiff::new(path, "generic default presence differs")),
    }
}

fn type_kind_diff(left: &Type, right: &Type, path: &str) -> AstDiff {
    AstDiff::new(
        path,
        format!(
            "type kind differs: {} vs {}",
            type_kind(left),
            type_kind(right)
        ),
    )
}

fn type_kind(ty: &Type) -> &'static str {
    match ty {
        Type::Named(_) => "Named",
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
    left: &luck_ast::expr::TableConstructor,
    right: &luck_ast::expr::TableConstructor,
    path: &str,
) -> Result<(), AstDiff> {
    if left.fields.len() != right.fields.len() {
        return Err(AstDiff::new(
            path,
            format!(
                "table field count differs: {} vs {}",
                left.fields.len(),
                right.fields.len()
            ),
        ));
    }
    for (idx, (lf, rf)) in left.fields.iter().zip(right.fields.iter()).enumerate() {
        field_eq(lf, rf, &child(path, &format!("field[{idx}]")))?;
    }
    Ok(())
}

fn field_eq(left: &Field, right: &Field, path: &str) -> Result<(), AstDiff> {
    match (left, right) {
        (
            Field::Named {
                name: ln,
                value: lv,
                ..
            },
            Field::Named {
                name: rn,
                value: rv,
                ..
            },
        ) => {
            token_text_eq(ln, rn, &child(path, "name"))?;
            expr_eq(lv, rv, &child(path, "value"))
        }
        (
            Field::Bracketed {
                key: lk, value: lv, ..
            },
            Field::Bracketed {
                key: rk, value: rv, ..
            },
        ) => {
            expr_eq(lk, rk, &child(path, "key"))?;
            expr_eq(lv, rv, &child(path, "value"))
        }
        (Field::Positional { value: lv, .. }, Field::Positional { value: rv, .. }) => {
            expr_eq(lv, rv, path)
        }
        _ => Err(AstDiff::new(path, "field kind differs")),
    }
}

fn punctuated_eq<T>(
    left: &Punctuated<T>,
    right: &Punctuated<T>,
    path: &str,
    mut item_eq: impl FnMut(&T, &T, &str) -> Result<(), AstDiff>,
) -> Result<(), AstDiff> {
    let left_len = left.len();
    let right_len = right.len();
    if left_len != right_len {
        return Err(AstDiff::new(
            path,
            format!("punctuated length differs: {left_len} vs {right_len}"),
        ));
    }
    for (idx, (la, ra)) in left.iter().zip(right.iter()).enumerate() {
        item_eq(la, ra, &child(path, &format!("[{idx}]")))?;
    }
    Ok(())
}

fn attributed_name_eq(
    left: &luck_ast::stmt::AttributedName,
    right: &luck_ast::stmt::AttributedName,
    path: &str,
) -> Result<(), AstDiff> {
    token_text_eq(&left.name, &right.name, path)?;
    type_annotation_eq(
        &left.type_annotation,
        &right.type_annotation,
        &child(path, "type"),
    )?;
    match (&left.attrib, &right.attrib) {
        (None, None) => Ok(()),
        (Some(l), Some(r)) => token_text_eq(&l.name, &r.name, &child(path, "attrib")),
        _ => Err(AstDiff::new(path, "attribute presence differs")),
    }
}

fn token_text_eq(
    left: &luck_token::Token,
    right: &luck_token::Token,
    path: &str,
) -> Result<(), AstDiff> {
    if left.kind != right.kind {
        return Err(AstDiff::new(path, "token kind differs"));
    }
    // Identifiers carry their text on the token via the source slice in
    // separate contexts; here we conservatively require identical kind only.
    // The full text comparison is performed via the source slice path during
    // verify (re-parsed buffer is the formatter's output, so identifiers must
    // match by definition if kinds match).
    Ok(())
}

/// Compare two raw number texts tolerating the only rewrites the formatter
/// performs: lowercasing the base prefix / exponent marker, case-folding
/// hex digits, and prepending a zero to a bare leading dot (`.5` -> `0.5`).
/// ASCII-lowercasing both raw texts (after undoing the leading-dot rewrite)
/// equates exactly those and nothing looser - `0x10` and `16` stay distinct
/// because the formatter never converts between bases.
fn number_raw_eq(left_raw: &str, right_raw: &str) -> bool {
    let strip_bare_zero = |raw: &str| -> String {
        match raw.strip_prefix("0.") {
            Some(rest) => format!(".{rest}"),
            None => raw.to_string(),
        }
    };
    strip_bare_zero(left_raw).eq_ignore_ascii_case(&strip_bare_zero(right_raw))
}

/// Compare two RAW string literal texts (quotes and escapes included) by
/// content, so the formatter's quote-style swap (`'x'` -> `"x"`,
/// re-escaping as needed) is tolerated.
fn string_raw_eq(left_raw: &str, right_raw: &str) -> bool {
    match (
        decode_string_literal(left_raw),
        decode_string_literal(right_raw),
    ) {
        (Some(left_value), Some(right_value)) => left_value == right_value,
        // Malformed literal on either side: fall back to raw equality
        _ => left_raw == right_raw,
    }
}

/// Singleton tokens mix fixed spellings (`nil`/`true`/`false`) with payload
/// literals; string payloads compare by decoded content, everything else by
/// kind.
fn singleton_eq(left: &Token, right: &Token) -> bool {
    match (&left.kind, &right.kind) {
        (
            luck_token::TokenKind::StringLiteral(left_raw),
            luck_token::TokenKind::StringLiteral(right_raw),
        ) => string_raw_eq(left_raw, right_raw),
        _ => left.kind == right.kind,
    }
}

/// Decode a raw Lua string literal (short quoted form or long bracket form)
/// to its byte contents. Returns `None` for malformed input.
fn decode_string_literal(raw: &str) -> Option<Vec<u8>> {
    let bytes = raw.as_bytes();
    match bytes.first()? {
        b'"' | b'\'' => decode_short_string(bytes),
        b'[' => decode_long_string(raw),
        _ => None,
    }
}

fn decode_short_string(bytes: &[u8]) -> Option<Vec<u8>> {
    let quote = bytes[0];
    if bytes.len() < 2 || *bytes.last()? != quote {
        return None;
    }
    let mut contents = Vec::with_capacity(bytes.len());
    let mut index = 1;
    let end = bytes.len() - 1;
    while index < end {
        let byte = bytes[index];
        if byte != b'\\' {
            contents.push(byte);
            index += 1;
            continue;
        }
        index += 1;
        let escape = *bytes.get(index)?;
        index += 1;
        match escape {
            b'n' => contents.push(b'\n'),
            b't' => contents.push(b'\t'),
            b'r' => contents.push(b'\r'),
            b'a' => contents.push(0x07),
            b'b' => contents.push(0x08),
            b'f' => contents.push(0x0c),
            b'v' => contents.push(0x0b),
            b'\\' | b'"' | b'\'' | b'\n' => contents.push(escape),
            // `\z` skips following whitespace
            b'z' => {
                while index < end && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
            }
            b'x' => {
                let hex = std::str::from_utf8(bytes.get(index..index + 2)?).ok()?;
                contents.push(u8::from_str_radix(hex, 16).ok()?);
                index += 2;
            }
            b'u' => {
                // `\u{XXXX}` - encode the codepoint as UTF-8
                if *bytes.get(index)? != b'{' {
                    return None;
                }
                index += 1;
                let close = bytes[index..end].iter().position(|&b| b == b'}')?;
                let hex = std::str::from_utf8(&bytes[index..index + close]).ok()?;
                let codepoint = u32::from_str_radix(hex, 16).ok()?;
                let decoded = char::from_u32(codepoint)?;
                let mut buffer = [0u8; 4];
                contents.extend_from_slice(decoded.encode_utf8(&mut buffer).as_bytes());
                index += close + 1;
            }
            b'0'..=b'9' => {
                let mut value: u32 = (escape - b'0') as u32;
                let mut digits = 1;
                while digits < 3 && index < end && bytes[index].is_ascii_digit() {
                    value = value * 10 + (bytes[index] - b'0') as u32;
                    index += 1;
                    digits += 1;
                }
                contents.push(u8::try_from(value).ok()?);
            }
            // Undefined escapes only lex under 5.1, where they mean the
            // literal character. Decoding both sides the same way keeps the
            // comparison exact; returning None would fall back to raw
            // equality and misreport a plain quote swap as a mismatch.
            _ => contents.push(escape),
        }
    }
    Some(contents)
}

fn decode_long_string(raw: &str) -> Option<Vec<u8>> {
    let level = raw.bytes().skip(1).take_while(|&byte| byte == b'=').count();
    let open_len = level + 2;
    let close_len = level + 2;
    if raw.len() < open_len + close_len {
        return None;
    }
    let mut contents = &raw[open_len..raw.len() - close_len];
    // Lua drops a newline immediately after the opening bracket
    if let Some(stripped) = contents.strip_prefix("\r\n") {
        contents = stripped;
    } else if let Some(stripped) = contents.strip_prefix('\n') {
        contents = stripped;
    }
    Some(contents.as_bytes().to_vec())
}

fn stmt_kind(stmt: &Statement) -> &'static str {
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
        Statement::Goto(_) => "Goto",
        Statement::Label(_) => "Label",
        Statement::GlobalDeclaration(_) => "GlobalDeclaration",
        Statement::GlobalFunction(_) => "GlobalFunction",
        Statement::GlobalStar(_) => "GlobalStar",
        Statement::Break(_) => "Break",
        Statement::CompoundAssignment(_) => "CompoundAssignment",
        Statement::TypeDeclaration(_) => "TypeDeclaration",
        Statement::Error(_) => "Error",
    }
}

fn expr_kind(expr: &Expression) -> &'static str {
    match expr {
        Expression::Nil(_) => "Nil",
        Expression::False(_) => "False",
        Expression::True(_) => "True",
        Expression::Number(_) => "Number",
        Expression::StringLiteral(_) => "StringLiteral",
        Expression::VarArg(_) => "VarArg",
        Expression::FunctionDef(_) => "FunctionDef",
        Expression::Var(_) => "Var",
        Expression::FunctionCall(_) => "FunctionCall",
        Expression::Parenthesized(_) => "Parenthesized",
        Expression::TableConstructor(_) => "TableConstructor",
        Expression::BinaryOp(_) => "BinaryOp",
        Expression::UnaryOp(_) => "UnaryOp",
        Expression::IfExpression(_) => "IfExpression",
        Expression::InterpolatedString(_) => "InterpolatedString",
        Expression::TypeCast(_) => "TypeCast",
        Expression::Error(_) => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn parse(src: &str) -> luck_ast::shared::Block {
        let result = luck_parser::parse(src, LuaVersion::Lua54);
        assert!(result.errors.is_empty(), "parse error: {:?}", result.errors);
        result.block
    }

    fn parse_luau(src: &str) -> luck_ast::shared::Block {
        let result = luck_parser::parse(src, LuaVersion::Luau);
        assert!(result.errors.is_empty(), "parse error: {:?}", result.errors);
        result.block
    }

    #[test]
    fn equivalent_when_formatting_differs() {
        // Both forms produce the same two LocalAssignment statements; the
        // semicolon variant may also include an EmptyStatement that affects
        // counts. We test the stricter "same whitespace" pair here so the
        // expected invariant is unambiguous.
        let a = parse("local x = 1\nlocal y = 2");
        let b = parse("local x=1\nlocal y=2");
        assert!(blocks_equiv(&a, &b).is_ok());
    }

    #[test]
    fn paren_strip_is_equivalent() {
        let a = parse("local x = (1 + 2)");
        let b = parse("local x = 1 + 2");
        assert!(blocks_equiv(&a, &b).is_ok());
    }

    #[test]
    fn call_paren_normalization_is_equivalent() {
        let a = parse("print(\"hi\")");
        let b = parse("print\"hi\"");
        assert!(blocks_equiv(&a, &b).is_ok());
    }

    #[test]
    fn detects_renamed_identifier() {
        // Lua's TokenKind::Identifier carries the identifier text, so two
        // different names have distinct kinds and the check rejects them.
        let a = parse("local x = 1");
        let b = parse("local y = 1");
        let err = blocks_equiv(&a, &b).expect_err("rename should diverge");
        assert!(err.reason.contains("kind"), "got: {}", err.reason);
    }

    #[test]
    fn detects_missing_statement() {
        let a = parse("local x = 1\nlocal y = 2");
        let b = parse("local x = 1");
        let err = blocks_equiv(&a, &b).expect_err("should diverge");
        assert!(err.reason.contains("count"));
    }

    #[test]
    fn detects_kind_change() {
        let a = parse("local x = 1");
        let b = parse("x = 1");
        let err = blocks_equiv(&a, &b).expect_err("should diverge");
        assert!(err.reason.contains("kind"));
    }

    #[test]
    fn identical_type_aliases_are_equivalent() {
        let a = parse_luau("type Pair = { first: number, second: string }");
        let b = parse_luau("type Pair = { first: number, second: string }");
        assert!(blocks_equiv(&a, &b).is_ok());
    }

    #[test]
    fn string_singleton_quote_style_is_equivalent() {
        // The formatter may swap quote style; singletons compare by decoded
        // content, so `"on"` and `'on'` are equivalent.
        let a = parse_luau("type State = \"on\"");
        let b = parse_luau("type State = 'on'");
        assert!(blocks_equiv(&a, &b).is_ok());
    }

    #[test]
    fn redundant_type_parens_are_equivalent() {
        let a = parse_luau("local x: (number) = 1");
        let b = parse_luau("local x: number = 1");
        assert!(blocks_equiv(&a, &b).is_ok());
    }

    #[test]
    fn leading_union_pipe_is_equivalent() {
        // A leading `|` is trivia the formatter may add or remove.
        let a = parse_luau("type Value = | number | string");
        let b = parse_luau("type Value = number | string");
        assert!(blocks_equiv(&a, &b).is_ok());
    }

    #[test]
    fn detects_differing_annotation_type() {
        let a = parse_luau("local x: number = 1");
        let b = parse_luau("local x: string = 1");
        let err = blocks_equiv(&a, &b).expect_err("differing types should diverge");
        assert!(err.path.contains("type"), "got path: {}", err.path);
    }

    #[test]
    fn detects_dropped_annotation() {
        // The data-loss bug this rewrite fixes: an annotation present on one
        // side and absent on the other must be reported, never tolerated.
        let a = parse_luau("local x: number = 1");
        let b = parse_luau("local x = 1");
        let err = blocks_equiv(&a, &b).expect_err("dropped annotation should diverge");
        assert!(
            err.reason.contains("presence differs"),
            "got: {}",
            err.reason
        );
    }
}
