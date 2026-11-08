//! Per-variant node discriminants and the borrowed node view used by the
//! linter's flat node table and bucketed dispatch.

use crate::expr::Expression;
use crate::stmt::{LastStatement, Statement};

/// One variant per `Statement`, `LastStatement`, and `Expression` variant.
///
/// `of_stmt`, `of_last_stmt`, and `of_expr` match exhaustively, so a new AST
/// variant doesn't compile until it has a `NodeType`.
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
    TypeInstantiation,
    ErrorExpr,
    Return,
    LastBreak,
    LastContinue,
    ErrorLastStmt,
}

impl NodeType {
    /// `ErrorLastStmt` must stay the last variant for this to count them all.
    pub const COUNT: usize = Self::ErrorLastStmt.index() + 1;

    /// Discriminant value, for callers indexing bucket arrays.
    #[must_use]
    #[expect(
        clippy::as_conversions,
        reason = "a const fn has no other way to read a fieldless enum discriminant"
    )]
    pub const fn index(self) -> usize {
        self as usize
    }

    const fn of_stmt(stmt: &Statement) -> Self {
        match stmt {
            Statement::Assignment(_) => Self::Assignment,
            Statement::FunctionCall(_) => Self::FunctionCallStmt,
            Statement::DoBlock(_) => Self::DoBlock,
            Statement::WhileLoop(_) => Self::WhileLoop,
            Statement::RepeatLoop(_) => Self::RepeatLoop,
            Statement::IfStatement(_) => Self::IfStatement,
            Statement::NumericFor(_) => Self::NumericFor,
            Statement::GenericFor(_) => Self::GenericFor,
            Statement::FunctionDecl(_) => Self::FunctionDecl,
            Statement::LocalFunction(_) => Self::LocalFunction,
            Statement::LocalAssignment(_) => Self::LocalAssignment,
            Statement::EmptyStatement(_) => Self::EmptyStatement,
            // Lua 5.2+
            Statement::Goto(_) => Self::Goto,
            // Lua 5.2+
            Statement::Label(_) => Self::Label,
            // Lua 5.5
            Statement::GlobalDeclaration(_) => Self::GlobalDeclaration,
            // Lua 5.5
            Statement::GlobalFunction(_) => Self::GlobalFunction,
            // Lua 5.5
            Statement::GlobalStar(_) => Self::GlobalStar,
            // Lua 5.2+
            Statement::Break(_) => Self::Break,
            // Luau
            Statement::CompoundAssignment(_) => Self::CompoundAssignment,
            // Luau
            Statement::TypeDeclaration(_) => Self::TypeDeclaration,
            Statement::Error(_) => Self::ErrorStmt,
        }
    }

    const fn of_last_stmt(last: &LastStatement) -> Self {
        match last {
            LastStatement::Return(_) => Self::Return,
            LastStatement::Break(_) => Self::LastBreak,
            // Luau
            LastStatement::Continue(_) => Self::LastContinue,
            LastStatement::Error(_) => Self::ErrorLastStmt,
        }
    }

    const fn of_expr(expr: &Expression) -> Self {
        match expr {
            Expression::Nil(_) => Self::Nil,
            Expression::False(_) => Self::False,
            Expression::True(_) => Self::True,
            Expression::Number(_) => Self::Number,
            // Luau
            Expression::Integer(_) => Self::Integer,
            Expression::StringLiteral(_) => Self::StringLiteral,
            Expression::VarArg(_) => Self::VarArg,
            Expression::FunctionDef(_) => Self::FunctionDef,
            Expression::Var(_) => Self::Var,
            Expression::FunctionCall(_) => Self::FunctionCallExpr,
            Expression::Parenthesized(_) => Self::Parenthesized,
            Expression::TableConstructor(_) => Self::TableConstructor,
            Expression::BinaryOp(_) => Self::BinaryOp,
            Expression::UnaryOp(_) => Self::UnaryOp,
            // Luau
            Expression::IfExpression(_) => Self::IfExpression,
            // Luau
            Expression::InterpolatedString(_) => Self::InterpolatedString,
            // Luau
            Expression::TypeCast(_) => Self::TypeCast,
            // Luau
            Expression::TypeInstantiation(_) => Self::TypeInstantiation,
            Expression::Error(_) => Self::ErrorExpr,
        }
    }
}

/// Borrowed view of one AST node, as dispatched to lint rules.
#[derive(Clone, Copy, Debug)]
pub enum NodeKind<'ast> {
    Statement(&'ast Statement),
    LastStatement(&'ast LastStatement),
    Expression(&'ast Expression),
}

impl NodeKind<'_> {
    #[must_use]
    pub const fn node_type(self) -> NodeType {
        match self {
            NodeKind::Statement(stmt) => NodeType::of_stmt(stmt),
            NodeKind::LastStatement(last) => NodeType::of_last_stmt(last),
            NodeKind::Expression(expr) => NodeType::of_expr(expr),
        }
    }
}

const _: () = assert!(NodeType::COUNT <= 64, "NodeTypeSet holds one bit per NodeType in a u64");

/// Fixed-size set of [`NodeType`]s, const-constructible so lint rules can
/// declare the node types they run on as a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NodeTypeSet(u64);

impl NodeTypeSet {
    #[must_use]
    pub const fn new() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn from_types(types: &[NodeType]) -> Self {
        let mut set = Self::new();
        let mut remaining = types;
        while let [first, rest @ ..] = remaining {
            set.insert(*first);
            remaining = rest;
        }
        set
    }

    /// Membership by [`NodeType::index`], for callers walking bucket arrays.
    #[must_use]
    pub const fn has_index(self, type_index: usize) -> bool {
        self.0 & (1u64 << type_index) != 0
    }

    pub const fn insert(&mut self, node_type: NodeType) {
        self.0 |= 1u64 << node_type.index();
    }

    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_type_set_tracks_membership_and_overlap() {
        let binary_ops = NodeTypeSet::from_types(&[NodeType::BinaryOp]);
        assert!(binary_ops.has_index(NodeType::BinaryOp.index()));
        assert!(!binary_ops.has_index(NodeType::UnaryOp.index()));

        let mut present = NodeTypeSet::new();
        present.insert(NodeType::ErrorExpr);
        assert!(!present.intersects(binary_ops));
        present.insert(NodeType::BinaryOp);
        assert!(present.intersects(binary_ops));
    }
}
