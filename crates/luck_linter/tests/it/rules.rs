use luck_linter::{FixKind, RuleId};
use luck_token::{Span, Variant};

use crate::common::{enable, lint_bytes};

/// A Latin-1 source lints as Luau reads it, and an escape of one of its bytes
/// names the byte.
#[test]
fn flags_a_lenient_escape_of_a_byte_that_is_not_utf8() {
    let source = b"print('caf\\\xE9') -- d\xE9j\xE0";
    let config = enable(RuleId::BadStringEscape);
    let diags = lint_bytes(source, Variant::Standalone, &config, FixKind::empty());
    let found: Vec<(&str, &str, Span)> =
        diags.iter().map(|diag| (diag.code, &*diag.message, diag.span())).collect();
    assert_eq!(found, [("bad_string_escape", "invalid string escape `\\xe9`", Span::new(10, 12))]);
}
