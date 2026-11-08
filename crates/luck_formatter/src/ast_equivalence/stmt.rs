use core::ops::ControlFlow::{self, Continue};

use luck_ast::{
    Assignment, AttributedName, Block, CompoundAssignment, ElseIfClause, FunctionDecl, GenericFor,
    GlobalDeclaration, IfStatement, LastStatement, LocalAssignment, LocalFunction, NumericFor,
    Statement, TypeDeclaration, TypeDeclarationValue,
};
use luck_token::LuaVersion;

use super::expr::{expr_eq, function_call_eq, var_eq};
use super::function::{function_attributes_eq, function_body_eq, param_eq};
use super::types::{generics_eq, type_annotation_eq, type_eq};
use super::{AstDiff, at, at_index, differ, presence_eq, punctuated_eq, require, token_text_eq};

pub(super) fn block_eq(left: &Block, right: &Block, version: LuaVersion) -> ControlFlow<AstDiff> {
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
