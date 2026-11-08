use core::ops::ControlFlow::{self, Continue};

use luck_ast::{FunctionAttribute, FunctionBody, Parameter};
use luck_token::LuaVersion;

use super::expr::expr_eq;
use super::stmt::block_eq;
use super::types::{generics_eq, type_annotation_eq};
use super::{AstDiff, at, at_index, presence_eq, punctuated_eq, require, token_text_eq};

/// Attributes have to match in order, by name and arguments, since losing one
/// such as `@native` changes how the function is compiled.
pub(super) fn function_attributes_eq(
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

pub(super) fn function_body_eq(
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

pub(super) fn param_eq(
    left: &Parameter,
    right: &Parameter,
    version: LuaVersion,
) -> ControlFlow<AstDiff> {
    token_text_eq(&left.name, &right.name)?;
    at(
        "type_annotation",
        type_annotation_eq(left.type_annotation.as_ref(), right.type_annotation.as_ref(), version),
    )
}
