//! Per-variant node discriminants and the borrowed node view used by the
//! flat node table (`luck_semantic`) and the linter's bucketed dispatch.

use crate::expr::Expression;
use crate::stmt::{LastStatement, Statement};

/// One variant per `Statement`, `LastStatement`, and `Expression`
/// variant. `of_stmt`, `of_last_stmt`, and `of_expr` are exhaustive, so
/// adding an AST variant without a `NodeType` fails to compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum NodeType {
    Assignment,
    FunctionCallStmt,
    DoBlock,
    WhileLoop,
    RepeatLoop,
    IfStatement,
    NumericFor,
    GenericFor,
    FunctionDecl,
    LocalFunction,
    LocalAssignment,
    EmptyStatement,
    Goto,
    Label,
    GlobalDeclaration,
    GlobalFunction,
    GlobalStar,
    Break,
    CompoundAssignment,
    TypeDeclaration,
    ErrorStmt,
    Nil,
    False,
    True,
    Number,
    // Luau
    Integer,
    StringLiteral,
    VarArg,
    FunctionDef,
    Var,
    FunctionCallExpr,
    Parenthesized,
    TableConstructor,
    BinaryOp,
    UnaryOp,
    IfExpression,
    InterpolatedString,
    TypeCast,
    // Luau
    TypeInstantiation,
    ErrorExpr,
    Return,
    LastBreak,
    LastContinue,
    ErrorLastStmt,
}

impl NodeType {
    pub const COUNT: usize = NodeType::ErrorLastStmt as usize + 1;

    #[must_use]
    pub fn of_stmt(stmt: &Statement) -> Self {
        match stmt {
            Statement::Assignment(_) => NodeType::Assignment,
            Statement::FunctionCall(_) => NodeType::FunctionCallStmt,
            Statement::DoBlock(_) => NodeType::DoBlock,
            Statement::WhileLoop(_) => NodeType::WhileLoop,
            Statement::RepeatLoop(_) => NodeType::RepeatLoop,
            Statement::IfStatement(_) => NodeType::IfStatement,
            Statement::NumericFor(_) => NodeType::NumericFor,
            Statement::GenericFor(_) => NodeType::GenericFor,
            Statement::FunctionDecl(_) => NodeType::FunctionDecl,
            Statement::LocalFunction(_) => NodeType::LocalFunction,
            Statement::LocalAssignment(_) => NodeType::LocalAssignment,
            Statement::EmptyStatement(_) => NodeType::EmptyStatement,
            Statement::Goto(_) => NodeType::Goto,
            Statement::Label(_) => NodeType::Label,
            Statement::GlobalDeclaration(_) => NodeType::GlobalDeclaration,
            Statement::GlobalFunction(_) => NodeType::GlobalFunction,
            Statement::GlobalStar(_) => NodeType::GlobalStar,
            Statement::Break(_) => NodeType::Break,
            Statement::CompoundAssignment(_) => NodeType::CompoundAssignment,
            Statement::TypeDeclaration(_) => NodeType::TypeDeclaration,
            Statement::Error(_) => NodeType::ErrorStmt,
        }
    }

    #[must_use]
    pub fn of_last_stmt(last: &LastStatement) -> Self {
        match last {
            LastStatement::Return(_) => NodeType::Return,
            LastStatement::Break(_) => NodeType::LastBreak,
            LastStatement::Continue(_) => NodeType::LastContinue,
            LastStatement::Error(_) => NodeType::ErrorLastStmt,
        }
    }

    #[must_use]
    pub fn of_expr(expr: &Expression) -> Self {
        match expr {
            Expression::Nil(_) => NodeType::Nil,
            Expression::False(_) => NodeType::False,
            Expression::True(_) => NodeType::True,
            Expression::Number(_) => NodeType::Number,
            Expression::Integer(_) => NodeType::Integer, // Luau
            Expression::StringLiteral(_) => NodeType::StringLiteral,
            Expression::VarArg(_) => NodeType::VarArg,
            Expression::FunctionDef(_) => NodeType::FunctionDef,
            Expression::Var(_) => NodeType::Var,
            Expression::FunctionCall(_) => NodeType::FunctionCallExpr,
            Expression::Parenthesized(_) => NodeType::Parenthesized,
            Expression::TableConstructor(_) => NodeType::TableConstructor,
            Expression::BinaryOp(_) => NodeType::BinaryOp,
            Expression::UnaryOp(_) => NodeType::UnaryOp,
            Expression::IfExpression(_) => NodeType::IfExpression,
            Expression::InterpolatedString(_) => NodeType::InterpolatedString,
            Expression::TypeCast(_) => NodeType::TypeCast,
            Expression::TypeInstantiation(_) => NodeType::TypeInstantiation, // Luau
            Expression::Error(_) => NodeType::ErrorExpr,
        }
    }
}

/// Borrowed view of one AST node, as dispatched to lint rules.
#[derive(Clone, Copy)]
pub enum NodeKind<'ast> {
    Statement(&'ast Statement),
    LastStatement(&'ast LastStatement),
    Expression(&'ast Expression),
}

impl NodeKind<'_> {
    #[must_use]
    pub fn node_type(self) -> NodeType {
        match self {
            NodeKind::Statement(stmt) => NodeType::of_stmt(stmt),
            NodeKind::LastStatement(last) => NodeType::of_last_stmt(last),
            NodeKind::Expression(expr) => NodeType::of_expr(expr),
        }
    }
}

const WORD_COUNT: usize = NodeType::COUNT.div_ceil(64);

const _: () = assert!(NodeType::ErrorLastStmt as usize == NodeType::COUNT - 1);
const _: () = assert!(NodeType::COUNT <= WORD_COUNT * 64);

/// Fixed-size set of [`NodeType`]s, const-constructible so rules can
/// declare the node types they run on in a `static`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AstTypesBitset([u64; WORD_COUNT]);

impl AstTypesBitset {
    #[must_use]
    pub const fn new() -> Self {
        Self([0; WORD_COUNT])
    }

    #[must_use]
    pub const fn from_types(types: &[NodeType]) -> Self {
        let mut bitset = Self::new();
        let mut i = 0;
        while i < types.len() {
            bitset.set(types[i]);
            i += 1;
        }
        bitset
    }

    const fn index_and_mask(ty: NodeType) -> (usize, u64) {
        (ty as usize / 64, 1u64 << (ty as usize % 64))
    }

    #[must_use]
    pub const fn has(&self, ty: NodeType) -> bool {
        let (index, mask) = Self::index_and_mask(ty);
        (self.0[index] & mask) != 0
    }

    /// `has` by discriminant value, for callers indexing bucket arrays.
    #[must_use]
    pub const fn has_index(&self, type_index: usize) -> bool {
        (self.0[type_index / 64] & (1u64 << (type_index % 64))) != 0
    }

    pub const fn set(&mut self, ty: NodeType) {
        let (index, mask) = Self::index_and_mask(ty);
        self.0[index] |= mask;
    }

    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        let mut intersection = 0;
        for (a, b) in self.0.iter().zip(other.0.iter()) {
            intersection |= a & b;
        }
        intersection != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitset_set_and_intersect() {
        let binops = AstTypesBitset::from_types(&[NodeType::BinaryOp]);
        assert!(binops.has(NodeType::BinaryOp));
        assert!(!binops.has(NodeType::UnaryOp));

        let mut present = AstTypesBitset::new();
        present.set(NodeType::ErrorExpr);
        assert!(!present.intersects(&binops));
        present.set(NodeType::BinaryOp);
        assert!(present.intersects(&binops));
    }
}
