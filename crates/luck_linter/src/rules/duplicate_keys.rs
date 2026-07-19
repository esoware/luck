use std::collections::HashMap;

use luck_ast::shared::Field;
use luck_token::TokenKind;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct DuplicateKeys;

impl Rule for DuplicateKeys {
    fn name(&self) -> &'static str {
        "duplicate_keys"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "table constructor has duplicate keys"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

impl NodeRule for DuplicateKeys {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::TableConstructor]);
        Some(&TYPES)
    }
    fn on_expression(
        &self,
        expr: &luck_ast::expr::Expression,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::Expression::TableConstructor(table) = expr {
            let mut seen: HashMap<String, luck_token::Span> = HashMap::new();
            for (field, _) in &table.fields {
                let key_name = match field {
                    Field::Named { name, .. } => {
                        if let TokenKind::Identifier(n) = &name.kind {
                            Some((n.to_string(), name.span))
                        } else {
                            None
                        }
                    }
                    Field::Bracketed { key, .. } => {
                        if let luck_ast::Expression::StringLiteral(token) = key {
                            let text =
                                &ctx.source[token.span.start as usize..token.span.end as usize];
                            // Compare decoded VALUES: `["\97"]` and `["a"]`
                            // are the same key; raw text says otherwise.
                            luck_token::literal::decode_string_literal(text, ctx.semantic.version)
                                .and_then(|bytes| String::from_utf8(bytes).ok())
                                .map(|value| (value, token.span))
                        } else {
                            None
                        }
                    }
                    Field::Positional { .. } => None,
                };

                if let Some((name, span)) = key_name {
                    if let Some(prev_span) = seen.get(&name) {
                        out.push(
                            LintDiagnostic::new(
                                "duplicate_keys",
                                format!("duplicate key '{name}' in table constructor"),
                                span,
                            )
                            .with_help(format!(
                                "previous definition at offset {}",
                                prev_span.start
                            )),
                        );
                    } else {
                        seen.insert(name, span);
                    }
                }
            }
        }
    }
}
