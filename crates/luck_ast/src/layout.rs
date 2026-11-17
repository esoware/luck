use crate::expr::{
    BinaryLink, BinaryOp, ElseIfExprClause, Expression, Field, FunctionArgs, FunctionDef,
    IfExpression, InterpolatedSegment, InterpolatedString, MethodCall, ParenthesizedExpression,
    Suffix, SuffixedExpression, TableConstructor, TypeCast, UnaryOp, Var,
};
use crate::function::{FunctionAttribute, FunctionBody, Parameter, VarArgParam};
use crate::leaf::{Literal, Name};
use crate::stmt::{
    Assignment, Attribute, AttributedName, Block, CompoundAssignment, DoBlock, ElseClause,
    ElseIfClause, FunctionDecl, FunctionName, GenericFor, GlobalDeclaration, GlobalFunction,
    GlobalStar, GotoStatement, IfStatement, LabelStatement, LastStatement, LocalAssignment,
    LocalFunction, NumericFor, RepeatLoop, ReturnStatement, Statement, TypeDeclaration,
    TypeDeclarationValue, WhileLoop,
};
use crate::types::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenthesizedType, TableType, Type, TypeArgs,
    TypeField, TypePack, TypeofType, UnionType, VariadicType,
};

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
        Expression<'_> => 16,
        Statement<'_> => 16,
        Type<'_> => 16,
        Option<Expression<'_>> => 16,
        Option<Type<'_>> => 16,
        Name<'_> => 24,
        Literal<'_> => 24,
        Var<'_> => 16,
        SuffixedExpression<'_> => 40,
        Suffix<'_> => 32,
        MethodCall<'_> => 64,
        FunctionArgs<'_> => 32,
        BinaryOp<'_> => 40,
        BinaryLink<'_> => 24,
        UnaryOp<'_> => 32,
        ParenthesizedExpression<'_> => 24,
        TableConstructor<'_> => 32,
        Field<'_> => 48,
        FunctionDef<'_> => 120,
        IfExpression<'_> => 72,
        ElseIfExprClause<'_> => 40,
        InterpolatedString<'_> => 48,
        InterpolatedSegment<'_> => 40,
        TypeCast<'_> => 40,
        Block<'_> => 40,
        LastStatement<'_> => 16,
        Option<LastStatement<'_>> => 16,
        Assignment<'_> => 40,
        DoBlock<'_> => 48,
        WhileLoop<'_> => 64,
        RepeatLoop<'_> => 64,
        IfStatement<'_> => 128,
        ElseIfClause<'_> => 64,
        ElseClause<'_> => 48,
        NumericFor<'_> => 136,
        GenericFor<'_> => 80,
        FunctionName<'_> => 48,
        FunctionDecl<'_> => 168,
        LocalFunction<'_> => 152,
        Attribute<'_> => 32,
        AttributedName<'_> => 48,
        LocalAssignment<'_> => 48,
        GotoStatement<'_> => 32,
        LabelStatement<'_> => 32,
        ReturnStatement<'_> => 24,
        CompoundAssignment<'_> => 48,
        TypeDeclaration<'_> => 64,
        TypeDeclarationValue<'_> => 16,
        GlobalDeclaration<'_> => 40,
        GlobalFunction<'_> => 128,
        GlobalStar<'_> => 40,
        FunctionBody<'_> => 96,
        Parameter<'_> => 40,
        VarArgParam<'_> => 48,
        FunctionAttribute<'_> => 48,
        NamedType<'_> => 80,
        TypeArgs<'_> => 24,
        TypeofType<'_> => 24,
        TableType<'_> => 32,
        TypeField<'_> => 56,
        FunctionType<'_> => 48,
        FunctionTypeParam<'_> => 40,
        OptionalType<'_> => 24,
        UnionType<'_> => 32,
        IntersectionType<'_> => 32,
        ParenthesizedType<'_> => 24,
        TypePack<'_> => 24,
        VariadicType<'_> => 24,
        GenericPackType<'_> => 32,
        GenericTypeList<'_> => 24,
        GenericTypeParam<'_> => 56,
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
