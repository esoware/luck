//! Inlay hints. Currently provides parameter-name hints at call sites
//! for stdlib functions whose arguments are scalar literals.

#![allow(clippy::while_let_loop)]

use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Var};
use luck_ast::visitor::Visitor;
use luck_semantic::stdlib_model::{
    EntryKind, StdlibArgKind, StdlibFunction, StdlibParam, library_for,
};
use luck_token::TokenKind;
use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, InlayHintParams, Position};

use crate::backend::DocumentState;

#[must_use]
pub fn inlay_hints(doc: &DocumentState, params: &InlayHintParams) -> Vec<InlayHint> {
    let range = params.range;
    let mut collector = HintCollector {
        doc,
        range_start: doc.line_index.offset(&doc.text, range.start),
        range_end: doc.line_index.offset(&doc.text, range.end),
        out: Vec::new(),
    };
    collector.visit_block(&doc.parsed.block);
    collector.out
}

struct HintCollector<'a> {
    doc: &'a DocumentState,
    range_start: u32,
    range_end: u32,
    out: Vec<InlayHint>,
}

impl Visitor for HintCollector<'_> {
    fn visit_expression(&mut self, expr: &Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.try_emit(call);
        }
        self.walk_expression(expr);
    }
}

impl HintCollector<'_> {
    fn try_emit(&mut self, call: &FunctionCall) {
        if call.span.end < self.range_start || call.span.start > self.range_end {
            return;
        }
        let path = call_path(call);
        if path.is_empty() {
            return;
        }
        let path_refs: Vec<&str> = path.iter().map(String::as_str).collect();
        let environment = self.doc.target.stdlib_environment();
        let Some(entry) = library_for(self.doc.target.lua_version())
            .lookup_str(&path_refs)
            .filter(|entry| entry.available_in_luau(environment))
        else {
            return;
        };
        let EntryKind::Function(func) = &entry.kind else {
            return;
        };
        let args_exprs = arg_expressions(call);
        for (idx, arg) in args_exprs.iter().enumerate() {
            let Some(param) = func.params.get(idx) else {
                break;
            };
            if matches!(param.kind, StdlibArgKind::Vararg) {
                break;
            }
            if !self.should_hint_for(arg) {
                continue;
            }
            let Some(name) = param_label(param, func, idx) else {
                continue;
            };
            let span_start = arg.span().start;
            let pos = self.doc.line_index.position(&self.doc.text, span_start);
            self.out.push(InlayHint {
                position: Position {
                    line: pos.line,
                    character: pos.character,
                },
                label: InlayHintLabel::String(format!("{name}:")),
                kind: Some(InlayHintKind::PARAMETER),
                text_edits: None,
                tooltip: None,
                padding_left: None,
                padding_right: Some(true),
                data: None,
            });
        }
    }

    /// Hints appear on literal arguments only, matching what real
    /// `initializationOptions` plumbing would default to anyway.
    fn should_hint_for(&self, expr: &Expression) -> bool {
        matches!(
            expr,
            Expression::Nil(_)
                | Expression::True(_)
                | Expression::False(_)
                | Expression::Number(_)
                | Expression::StringLiteral(_)
        )
    }
}

fn param_label(param: &StdlibParam, _func: &StdlibFunction, idx: usize) -> Option<String> {
    // We don't store parameter names in the rich model. Synthesize a
    // useful label from the typed kind: `arg1: number`, `mode: "r"|"w"`, etc.
    match &param.kind {
        StdlibArgKind::Vararg => None,
        StdlibArgKind::Constant(values) => Some(format!(
            "arg{}: {}",
            idx + 1,
            values
                .iter()
                .map(|v| format!("\"{v}\""))
                .collect::<Vec<_>>()
                .join(" | ")
        )),
        StdlibArgKind::Display(d) => Some(format!("arg{}: {d}", idx + 1)),
        other => Some(format!("arg{}: {}", idx + 1, type_label(other))),
    }
}

fn type_label(kind: &StdlibArgKind) -> &'static str {
    match kind {
        StdlibArgKind::Any => "any",
        StdlibArgKind::Bool => "bool",
        StdlibArgKind::Number => "number",
        StdlibArgKind::String => "string",
        StdlibArgKind::Function => "function",
        StdlibArgKind::Table => "table",
        StdlibArgKind::Nil => "nil",
        StdlibArgKind::Constant(_) => "constant",
        StdlibArgKind::Display(_) => "value",
        StdlibArgKind::Vararg => "...",
    }
}

fn arg_expressions(call: &FunctionCall) -> Vec<&Expression> {
    match &call.args {
        FunctionArgs::Parenthesized { args, .. } => args.iter().collect(),
        FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => Vec::new(),
    }
}

fn call_path(call: &FunctionCall) -> Vec<String> {
    if call.method.is_some() {
        return Vec::new();
    }
    let Expression::Var(var) = &call.callee else {
        return Vec::new();
    };
    match var.as_ref() {
        Var::Name(token) => match &token.kind {
            TokenKind::Identifier(name) => vec![name.to_string()],
            _ => Vec::new(),
        },
        Var::FieldAccess(fa) => {
            let mut segments: Vec<String> = Vec::new();
            if let TokenKind::Identifier(name) = &fa.name.kind {
                segments.push(name.to_string());
            }
            let mut cursor: &Expression = &fa.prefix;
            loop {
                match cursor {
                    Expression::Var(inner) => match inner.as_ref() {
                        Var::Name(token) => {
                            if let TokenKind::Identifier(name) = &token.kind {
                                segments.push(name.to_string());
                            }
                            break;
                        }
                        Var::FieldAccess(inner_fa) => {
                            if let TokenKind::Identifier(name) = &inner_fa.name.kind {
                                segments.push(name.to_string());
                            } else {
                                break;
                            }
                            cursor = &inner_fa.prefix;
                        }
                        Var::Index(_) => break,
                    },
                    _ => break,
                }
            }
            segments.reverse();
            segments
        }
        Var::Index(_) => Vec::new(),
    }
}
