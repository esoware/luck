use luck_ast::{Expression, Literal};
use luck_token::LuaVersion;

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::number::{IntegerDigits, integer_digits};
use crate::rule::{LintContext, NodeRule, impl_rule};

/// Lua 5.1, 5.2, and Luau store every number as a double, so integers above
/// 2^53 can round.
///
/// Lua 5.3+ has 64-bit integers: a decimal literal above 2^63-1 becomes a
/// float, and a hex literal above 2^64-1 wraps around.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IntegerParsing;

impl_rule!(
    IntegerParsing,
    "integer_parsing",
    Correctness,
    Warning,
    "integer literal the target Lua version can't represent exactly"
);

impl NodeRule for IntegerParsing {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::Number])
    }
    fn on_expr(&self, expr: &Expression, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        if let Expression::Number(literal) = expr {
            findings.extend(check_number(literal, context.semantic.version));
        }
    }
}

fn check_number(literal: &Literal, version: LuaVersion) -> Option<Finding> {
    let raw = literal.text.as_str();
    // Float spellings go unchecked, since nobody writes one expecting an
    // exact integer.
    let IntegerDigits { digits, radix } = integer_digits(raw)?;
    let magnitude = u128::from_str_radix(&digits, radix);
    let (message, help) = if !version.has_integer_subtype() {
        // Without an integer subtype every number is a double. Text too
        // large for `u128` counts as inexact.
        if magnitude.is_ok_and(is_exact_in_double) {
            return None;
        }
        (
            format!(
                "integer literal `{raw}` exceeds 2^53, so it loses precision when stored as a double"
            ),
            "use a smaller value or store as a string",
        )
    } else if radix == 10 {
        // Lua 5.3+ reads a decimal literal above `i64::MAX` as a float.
        if digits.parse::<i64>().is_ok() {
            return None;
        }
        (
            format!(
                "decimal integer `{raw}` exceeds 2^63-1, so it will be parsed as a float and lose precision"
            ),
            "use a hex literal or split into smaller values",
        )
    } else {
        // Lua 5.3+ wraps a hex literal that overflows 64 bits.
        if magnitude.is_ok_and(|v| v <= u128::from(u64::MAX)) {
            return None;
        }
        (
            format!(
                "hex literal `{raw}` exceeds the 64-bit integer range, so it wraps around to a different value"
            ),
            "use a smaller value or split across two values",
        )
    };
    Some(Finding::new(message, literal.span).with_help(help))
}

/// Whether a double holds `value` exactly, meaning its significant bits fit
/// the 53-bit mantissa.
const fn is_exact_in_double(value: u128) -> bool {
    value.wrapping_shr(value.trailing_zeros()) < (1u128 << 53)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        run_rule(&IntegerParsing, source, version)
    }

    #[test]
    fn flags_hex_over_64bits_on_53() {
        let diags = run("local x = 0xFFFFFFFFFFFFFFFFFF", LuaVersion::Lua53);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_small_hex() {
        let diags = run("local x = 0xFF", LuaVersion::Lua53);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_decimal_above_2pow53_on_51() {
        let diags = run("local x = 9007199254740993", LuaVersion::Lua51);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_small_decimal() {
        let diags = run("local x = 100", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_float_literal() {
        let diags = run("local x = 1.5", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_decimal_above_i64_on_54() {
        let diags = run("local x = 99999999999999999999", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_underscore_separated_literal_on_luau() {
        let diags = run("local x = 1_000", LuaVersion::Luau);
        assert!(diags.is_empty(), "1000 is exact in a double: {diags:?}");
    }

    #[test]
    fn ignores_binary_literal_on_luau() {
        let diags = run("local x = 0b101", LuaVersion::Luau);
        assert!(diags.is_empty(), "5 is exact in a double: {diags:?}");
    }

    #[test]
    fn ignores_exactly_representable_integer_on_lua51() {
        let diags = run("local x = 9007199254740994", LuaVersion::Lua51);
        assert!(diags.is_empty(), "2^53 + 2 is exact in a double: {diags:?}");
    }
}
