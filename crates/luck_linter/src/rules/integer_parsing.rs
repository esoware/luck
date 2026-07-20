use luck_ast::Expression;
use luck_ast::node::{AstTypesBitset, NodeType};
use luck_token::{LuaVersion, Span};

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};

/// Numeric literals that exceed the precision of the target runtime's
/// number representation. On Lua 5.1/5.2 every number is an IEEE-754
/// double, so integers above 2^53 silently round. On 5.3+ integers are
/// 64-bit, so hex literals above 2^64-1 cannot be represented at all.
pub struct IntegerParsing;

impl Rule for IntegerParsing {
    fn name(&self) -> &'static str {
        "integer_parsing"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "integer literal exceeds the target Lua version's representable range"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

struct LiteralChecker<'a> {
    version: LuaVersion,
    out: &'a mut Vec<LintDiagnostic>,
}

/// Whether the runtime treats integers as a distinct 64-bit subtype.
fn has_64bit_integers(version: LuaVersion) -> bool {
    matches!(
        version,
        LuaVersion::Lua53 | LuaVersion::Lua54 | LuaVersion::Lua55
    )
}

/// Number text classification. Floats (anything with `.`, `p`, `P`, `e`,
/// or `E` in the relevant position) are not integers and we let them
/// through - even though they lose precision differently, that's a
/// separate concern from "is this *intended* as an exact integer".
enum LiteralShape<'a> {
    Hex(&'a str),
    Decimal(&'a str),
    NotInteger,
}

fn classify(text: &str) -> LiteralShape<'_> {
    if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        // Hex floats (`0x1.Fp10`) and explicit exponents are not integers.
        if rest.bytes().any(|byte| matches!(byte, b'.' | b'p' | b'P')) {
            return LiteralShape::NotInteger;
        }
        if rest.is_empty() {
            return LiteralShape::NotInteger;
        }
        return LiteralShape::Hex(rest);
    }
    if text.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E')) {
        return LiteralShape::NotInteger;
    }
    if text.is_empty() {
        return LiteralShape::NotInteger;
    }
    LiteralShape::Decimal(text)
}

impl LiteralChecker<'_> {
    fn check_number(&mut self, span: Span, raw: &str) {
        match classify(raw) {
            LiteralShape::Hex(digits) => {
                if !has_64bit_integers(self.version) {
                    // On 5.1/5.2 every literal is a double; apply the
                    // 2^53 ceiling using the parsed value.
                    if let Ok(value) = u128::from_str_radix(digits, 16) {
                        if value > (1u128 << 53) {
                            self.push_double_precision(span, raw);
                        }
                    } else {
                        // Too large even for u128 -> clearly over budget.
                        self.push_double_precision(span, raw);
                    }
                    return;
                }
                // On 5.3+, integers are 64-bit. Anything that overflows
                // u64 wraps modulo 2^64 in the reference implementation.
                match u128::from_str_radix(digits, 16) {
                    Ok(value) if value > u128::from(u64::MAX) => {
                        self.push_hex_overflow(span, raw);
                    }
                    Err(_) => {
                        self.push_hex_overflow(span, raw);
                    }
                    Ok(_) => {}
                }
            }
            LiteralShape::Decimal(digits) => {
                if has_64bit_integers(self.version) {
                    // Decimal literals exceeding i64::MAX become floats
                    // in 5.3+ (per `lvm` parsing rules). Flag them since
                    // a plain integer literal was clearly intended.
                    let parsed = digits.parse::<u128>();
                    let above = match parsed {
                        Ok(value) => value > i64::MAX as u128,
                        Err(_) => true,
                    };
                    if above {
                        self.out.push(LintDiagnostic::new("integer_parsing", format!(
                                "decimal integer `{raw}` exceeds 2^63-1; will be parsed as a float and lose precision"
                            ), span).with_help(
                                "use a hex literal or split into smaller values".to_string(),
                            ));
                    }
                } else {
                    let parsed = digits.parse::<u128>();
                    let above = match parsed {
                        Ok(value) => value > (1u128 << 53),
                        Err(_) => true,
                    };
                    if above {
                        self.push_double_precision(span, raw);
                    }
                }
            }
            LiteralShape::NotInteger => {}
        }
    }

    fn push_double_precision(&mut self, span: Span, raw: &str) {
        self.out.push(
            LintDiagnostic::new(
                "integer_parsing",
                format!(
                    "integer literal `{raw}` exceeds 2^53; loses precision when stored as a double"
                ),
                span,
            )
            .with_help("use a smaller value or store as a string".to_string()),
        );
    }

    fn push_hex_overflow(&mut self, span: Span, raw: &str) {
        self.out.push(LintDiagnostic::new("integer_parsing", format!(
                "hex literal `{raw}` exceeds 64-bit integer range; will wrap or be parsed as a float"
            ), span).with_help("use a smaller value or split across two values".to_string()));
    }
}

impl NodeRule for IntegerParsing {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::Number]);
        Some(&TYPES)
    }
    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::Number(literal) = expr {
            LiteralChecker {
                version: ctx.semantic.version,
                out,
            }
            .check_number(literal.span, &literal.text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&IntegerParsing, source, version)
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
}
