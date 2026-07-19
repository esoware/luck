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
//! let mut synth = Synth::new();
//! let expr = synth.number("42");
//! assert!(matches!(expr, Expression::Number(_)));
//! ```

pub mod builder;
pub mod expr;
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
            expr::FunctionCall => 240,
            expr::FunctionArgs => 104,
            expr::BinaryOp => 144,
            expr::UnaryOp => 96,
            expr::ParenExpression => 136,
            expr::TableConstructor => 112,
            expr::IndexExpression => 184,
            expr::FieldAccess => 136,
            expr::FunctionDef => 496,
            expr::IfExpression => 296,
            expr::ElseIfExprClause => 184,
            expr::InterpolatedString => 32,
            expr::InterpSegment => 88,
            expr::TypeCast => 136,
            Statement => 48,
            LastStatement => 48,
            stmt::Assignment => 96,
            stmt::FunctionCallStmt => 248,
            stmt::DoBlock => 128,
            stmt::WhileLoop => 216,
            stmt::RepeatLoop => 176,
            stmt::IfStatement => 328,
            stmt::ElseIfClause => 176,
            stmt::ElseClause => 88,
            stmt::NumericFor => 552,
            stmt::GenericFor => 256,
            stmt::FuncName => 136,
            stmt::FunctionAttribute => 88,
            stmt::FunctionDecl => 656,
            stmt::LocalFunction => 600,
            stmt::Attribute => 128,
            stmt::AttributedName => 248,
            stmt::LocalAssignment => 136,
            stmt::GotoStatement => 88,
            stmt::LabelStatement => 128,
            stmt::ReturnStatement => 112,
            stmt::CompoundAssignment => 136,
            stmt::TypeDeclaration => 256,
            stmt::TypeDeclarationValue => 40,
            stmt::GlobalDeclaration => 72,
            stmt::GlobalFunction => 576,
            stmt::GlobalStar => 216,
            Type => 40,
            types::NamedType => 240,
            types::TypeArgs => 112,
            types::TypeofType => 176,
            types::TableType => 112,
            types::TypeField => 248,
            types::FunctionType => 304,
            types::FunctionTypeParam => 128,
            types::OptionalType => 88,
            types::UnionType => 72,
            types::IntersectionType => 72,
            types::ParenType => 128,
            types::TypePack => 112,
            types::VariadicType => 88,
            types::GenericPackType => 88,
            types::GenericTypeList => 112,
            types::GenericTypeParam => 168,
            ContainedSpan => 80,
            FunctionBody => 448,
            Parameter => 128,
            VarArgParam => 168,
            Block => 40,
            Field => 224,
        }
    }
}
