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

pub use expr::Expression;
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
            Expression => 48,
            expr::Var => 40,
            expr::FunctionCall => 152,
            expr::FunctionArgs => 48,
            expr::BinaryOp => 120,
            expr::UnaryOp => 72,
            expr::ParenExpression => 72,
            expr::TableConstructor => 48,
            expr::IndexExpression => 120,
            expr::FieldAccess => 104,
            expr::FunctionDef => 296,
            expr::IfExpression => 200,
            expr::ElseIfExprClause => 120,
            expr::InterpolatedString => 32,
            expr::InterpSegment => 88,
            expr::TypeCast => 104,
            Statement => 16,
            LastStatement => 16,
            stmt::Assignment => 64,
            stmt::FunctionCallStmt => 160,
            stmt::DoBlock => 64,
            stmt::WhileLoop => 120,
            stmt::RepeatLoop => 112,
            stmt::IfStatement => 200,
            stmt::ElseIfClause => 112,
            stmt::ElseClause => 56,
            stmt::NumericFor => 328,
            stmt::GenericFor => 128,
            stmt::FuncName => 104,
            stmt::FunctionAttribute => 80,
            stmt::FunctionDecl => 400,
            stmt::LocalFunction => 352,
            stmt::Attribute => 64,
            stmt::AttributedName => 152,
            stmt::LocalAssignment => 80,
            stmt::GotoStatement => 56,
            stmt::LabelStatement => 64,
            stmt::ReturnStatement => 56,
            stmt::CompoundAssignment => 112,
            stmt::TypeDeclaration => 144,
            stmt::TypeDeclarationValue => 40,
            stmt::GlobalDeclaration => 72,
            stmt::GlobalFunction => 320,
            stmt::GlobalStar => 88,
            Type => 40,
            types::NamedType => 144,
            types::TypeArgs => 48,
            types::TypeofType => 80,
            types::TableType => 48,
            types::TypeField => 152,
            types::FunctionType => 144,
            types::FunctionTypeParam => 96,
            types::OptionalType => 56,
            types::UnionType => 48,
            types::IntersectionType => 48,
            types::ParenType => 64,
            types::TypePack => 48,
            types::VariadicType => 56,
            types::GenericPackType => 56,
            types::GenericTypeList => 48,
            types::GenericTypeParam => 112,
            ContainedSpan => 16,
            FunctionBody => 256,
            Parameter => 96,
            VarArgParam => 104,
            Block => 40,
            Field => 128,
        }
    }
}
