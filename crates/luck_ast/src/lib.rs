//! # luck_ast
//!
//! AST node types and traversal infrastructure for Lua/Luau.
//!
//! ## Key Types
//!
//! - [`Expression`] - All expression variants (binary, unary, call, index, literal, etc.)
//! - [`Statement`] - All statement variants (assign, local, if, while, for, etc.)
//! - [`Block`] - A sequence of statements with an optional last statement
//!
//! ## Traversal
//!
//! - [`Visitor`](visitor::Visitor) - Read-only AST traversal
//! - [`AstTransform`](transform::AstTransform) - Mutable `fn(Node) -> Node` transforms (used by the minifier)
//!
//! Both patterns: override `visit_*`/`transform_*` methods, call `self.walk_*` for default recursion.
//!
//! # Usage
//!
//! ```
//! use luck_ast::Expression;
//! use luck_ast::synth::Synth;
//!
//! let synth = Synth::new();
//! let expr = synth.number("42");
//! assert!(matches!(expr, Expression::Number(_)));
//! ```

pub mod builder;
pub mod expr;
pub mod node;
pub mod query;
pub mod shared;
pub mod stmt;
pub mod synth;
pub mod transform;
pub mod types;
pub mod visitor;

pub use expr::{Expression, Literal};
pub use shared::*;
pub use stmt::{LastStatement, Statement};
pub use types::Type;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ast_enum_sizes() {
        assert!(
            std::mem::size_of::<Expression>() <= 64,
            "Expression enum is {} bytes, should be <= 64",
            std::mem::size_of::<Expression>()
        );
        assert!(
            std::mem::size_of::<Statement>() <= 64,
            "Statement enum is {} bytes, should be <= 64",
            std::mem::size_of::<Statement>()
        );
        assert!(
            std::mem::size_of::<Type>() <= 64,
            "Type enum is {} bytes, should be <= 64",
            std::mem::size_of::<Type>()
        );
    }

    // Pins the size of every AST node so a field addition that balloons a
    // node fails loudly with the numbers instead of silently regressing
    // memory. On mismatch, paste the printed rows back into the table.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn ast_node_layouts() {
        macro_rules! check_layouts {
            ($($node:ty => $expected:expr,)*) => {{
                let mut mismatches = Vec::new();
                $(
                    let actual = std::mem::size_of::<$node>();
                    if actual != $expected {
                        mismatches.push(format!(
                            concat!("        ", stringify!($node), " => {},"),
                            actual
                        ));
                    }
                )*
                assert!(
                    mismatches.is_empty(),
                    "node sizes changed; update the table:\n{}",
                    mismatches.join("\n")
                );
            }};
        }
        check_layouts! {
            Expression => 40,
            expr::Var => 40,
            expr::FunctionCall => 128,
            expr::FunctionArgs => 40,
            expr::BinaryOp => 96,
            expr::UnaryOp => 56,
            expr::ParenExpression => 48,
            expr::TableConstructor => 40,
            expr::IndexExpression => 88,
            expr::FieldAccess => 88,
            expr::FunctionDef => 248,
            expr::IfExpression => 152,
            expr::ElseIfExprClause => 88,
            expr::InterpolatedString => 32,
            expr::InterpSegment => 80,
            expr::TypeCast => 88,
            Statement => 16,
            LastStatement => 16,
            stmt::Assignment => 72,
            stmt::FunctionCallStmt => 136,
            stmt::DoBlock => 48,
            stmt::WhileLoop => 88,
            stmt::RepeatLoop => 88,
            stmt::IfStatement => 160,
            stmt::ElseIfClause => 88,
            stmt::ElseClause => 48,
            stmt::NumericFor => 248,
            stmt::GenericFor => 112,
            stmt::FuncName => 72,
            stmt::FunctionAttribute => 80,
            stmt::FunctionDecl => 320,
            stmt::LocalFunction => 296,
            stmt::Attribute => 48,
            stmt::AttributedName => 128,
            stmt::LocalAssignment => 80,
            stmt::GotoStatement => 48,
            stmt::LabelStatement => 48,
            stmt::ReturnStatement => 40,
            stmt::CompoundAssignment => 96,
            stmt::TypeDeclaration => 104,
            stmt::TypeDeclarationValue => 40,
            stmt::GlobalDeclaration => 72,
            stmt::GlobalFunction => 264,
            stmt::GlobalStar => 56,
            Type => 40,
            types::NamedType => 128,
            types::TypeArgs => 40,
            types::TypeofType => 48,
            types::TableType => 40,
            types::TypeField => 136,
            types::FunctionType => 120,
            types::FunctionTypeParam => 88,
            types::OptionalType => 48,
            types::UnionType => 48,
            types::IntersectionType => 48,
            types::ParenType => 48,
            types::TypePack => 40,
            types::VariadicType => 48,
            types::GenericPackType => 48,
            types::GenericTypeList => 40,
            types::GenericTypeParam => 96,
            FunctionBody => 216,
            Parameter => 88,
            VarArgParam => 88,
            Block => 40,
            Field => 96,
        }
    }
}
