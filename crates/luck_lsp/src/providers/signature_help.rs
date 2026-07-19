//! Signature help. Finds the enclosing call expression at the cursor,
//! looks the callee up in the stdlib model, and renders one signature
//! with the active-parameter index set to the comma-count.

use luck_semantic::stdlib_model::{EntryKind, StdlibArgKind, StdlibFunction, library_for};
use tower_lsp::lsp_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, SignatureHelp,
    SignatureHelpParams, SignatureInformation,
};

use crate::backend::DocumentState;
use crate::providers::cursor::find_call_site_at;

#[must_use]
pub fn signature_help(doc: &DocumentState, params: &SignatureHelpParams) -> Option<SignatureHelp> {
    let position = params.text_document_position_params.position;
    let offset = doc.line_index.offset(&doc.text, position);
    let call = find_call_site_at(&doc.parsed.block, &doc.text, offset)?;
    let path: Vec<&str> = call.path.iter().map(String::as_str).collect();
    if path.is_empty() {
        return None;
    }
    let environment = doc.target.stdlib_environment();
    let entry = library_for(doc.target.lua_version())
        .lookup_str(&path)
        .filter(|entry| entry.available_in_luau(environment))?;
    let EntryKind::Function(func) = &entry.kind else {
        return None;
    };

    let signature = build_signature(&path.join("."), func);
    let active_param = clamp_active_param(call.active_param, func);

    Some(SignatureHelp {
        signatures: vec![signature],
        active_signature: Some(0),
        active_parameter: Some(active_param),
    })
}

fn clamp_active_param(active: u32, func: &StdlibFunction) -> u32 {
    let max = if func.params.is_empty() {
        0
    } else {
        (func.params.len() - 1) as u32
    };
    active.min(max)
}

fn build_signature(name: &str, func: &StdlibFunction) -> SignatureInformation {
    let mut label = format!("{name}(");
    let mut parameters: Vec<ParameterInformation> = Vec::new();
    for (idx, param) in func.params.iter().enumerate() {
        if idx > 0 {
            label.push_str(", ");
        }
        let start = label.chars().count() as u32;
        let rendered = match &param.kind {
            StdlibArgKind::Vararg => "...".to_string(),
            kind => {
                let base = match kind {
                    StdlibArgKind::Any => "any",
                    StdlibArgKind::Bool => "bool",
                    StdlibArgKind::Number => "number",
                    StdlibArgKind::String => "string",
                    StdlibArgKind::Function => "function",
                    StdlibArgKind::Table => "table",
                    StdlibArgKind::Nil => "nil",
                    StdlibArgKind::Display(d) => d.as_str(),
                    StdlibArgKind::Constant(_) => "constant",
                    StdlibArgKind::Vararg => unreachable!(),
                };
                if param.required {
                    base.to_string()
                } else {
                    format!("{base}?")
                }
            }
        };
        label.push_str(&rendered);
        let end = label.chars().count() as u32;
        parameters.push(ParameterInformation {
            label: ParameterLabel::LabelOffsets([start, end]),
            documentation: param_doc(param),
        });
    }
    label.push(')');

    let mut documentation = String::new();
    if let Some(dep) = &func.deprecated {
        documentation.push_str(&format!("**Deprecated:** {}\n", dep.message));
    }
    if func.must_use {
        documentation.push_str("\n_Return value should not be discarded._");
    }

    SignatureInformation {
        label,
        documentation: if documentation.is_empty() {
            None
        } else {
            Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: documentation,
            }))
        },
        parameters: Some(parameters),
        active_parameter: None,
    }
}

fn param_doc(param: &luck_semantic::stdlib_model::StdlibParam) -> Option<Documentation> {
    if let StdlibArgKind::Constant(values) = &param.kind {
        let body = values
            .iter()
            .map(|v| format!("`\"{v}\"`"))
            .collect::<Vec<_>>()
            .join(", ");
        return Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("one of: {body}"),
        }));
    }
    if !param.required {
        return Some(Documentation::String("optional".to_string()));
    }
    None
}
