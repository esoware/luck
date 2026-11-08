use crate::expr::{
    BinaryOp, ElseIfExprClause, Expression, FieldAccess, FunctionArgs, FunctionCall, FunctionDef,
    IfExpression, IndexExpression, InterpolatedString, InterpolatedStringSegment,
    ParenthesizedExpression, TableConstructor, TypeCast, TypeInstantiation, UnaryOp, Var,
};
use crate::shared::{Block, Field, FunctionBody, Parameter, VarArgParam};
use crate::stmt::{
    Assignment, Attribute, AttributedName, CompoundAssignment, DoBlock, ElseClause, ElseIfClause,
    FunctionAttribute, FunctionDecl, FunctionName, GenericFor, GlobalDeclaration, GlobalFunction,
    GlobalStar, GotoStatement, IfStatement, LabelStatement, LastStatement, LocalAssignment,
    LocalFunction, NumericFor, RepeatLoop, ReturnStatement, Statement, TypeDeclaration,
    TypeDeclarationValue, WhileLoop,
};
use crate::types::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenthesizedType, TableType, Type, TypeArgs,
    TypeField, TypePack, TypeofType, UnionType, VariadicType,
};

#[test]
fn ast_enum_sizes() {
    assert!(
        size_of::<Expression>() <= 64,
        "Expression enum is {} bytes, should be <= 64",
        size_of::<Expression>()
    );
    assert!(
        size_of::<Statement>() <= 64,
        "Statement enum is {} bytes, should be <= 64",
        size_of::<Statement>()
    );
    assert!(size_of::<Type>() <= 64, "Type enum is {} bytes, should be <= 64", size_of::<Type>());
}

// Pins every node's size so a node can't grow unnoticed. On a mismatch,
// paste the printed rows back into the table.
#[test]
#[cfg(target_pointer_width = "64")]
fn ast_node_layouts() {
    macro_rules! layouts {
        ($($node:ty => $expected:expr,)*) => {
            [$((stringify!($node), size_of::<$node>(), $expected),)*]
        };
    }
    let layouts = layouts! {
        Expression => 40,
        Var => 40,
        FunctionCall => 136,
        FunctionArgs => 40,
        BinaryOp => 96,
        UnaryOp => 56,
        ParenthesizedExpression => 48,
        TableConstructor => 40,
        IndexExpression => 88,
        FieldAccess => 88,
        FunctionDef => 168,
        IfExpression => 152,
        ElseIfExprClause => 88,
        InterpolatedString => 32,
        InterpolatedStringSegment => 80,
        TypeCast => 88,
        TypeInstantiation => 88,
        Statement => 16,
        LastStatement => 16,
        Assignment => 72,
        DoBlock => 48,
        WhileLoop => 88,
        RepeatLoop => 88,
        IfStatement => 160,
        ElseIfClause => 88,
        ElseClause => 48,
        NumericFor => 248,
        GenericFor => 112,
        FunctionName => 72,
        FunctionAttribute => 80,
        FunctionDecl => 240,
        LocalFunction => 216,
        Attribute => 48,
        AttributedName => 88,
        LocalAssignment => 80,
        GotoStatement => 48,
        LabelStatement => 48,
        ReturnStatement => 40,
        CompoundAssignment => 96,
        TypeDeclaration => 104,
        TypeDeclarationValue => 40,
        GlobalDeclaration => 72,
        GlobalFunction => 184,
        GlobalStar => 56,
        Type => 40,
        NamedType => 128,
        TypeArgs => 40,
        TypeofType => 48,
        TableType => 40,
        TypeField => 136,
        FunctionType => 120,
        FunctionTypeParam => 88,
        OptionalType => 48,
        UnionType => 48,
        IntersectionType => 48,
        ParenthesizedType => 48,
        TypePack => 40,
        VariadicType => 48,
        GenericPackType => 48,
        GenericTypeList => 40,
        GenericTypeParam => 96,
        FunctionBody => 136,
        Parameter => 88,
        VarArgParam => 88,
        Block => 40,
        Field => 96,
    };
    let mismatches: Vec<String> = layouts
        .into_iter()
        .filter(|(_, actual, expected)| actual != expected)
        .map(|(node, actual, _)| format!("        {node} => {actual},"))
        .collect();
    assert!(
        mismatches.is_empty(),
        "node sizes changed; update the table:\n{}",
        mismatches.join("\n")
    );
}
