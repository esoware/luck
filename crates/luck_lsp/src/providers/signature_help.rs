//! Signature help. Finds the enclosing call expression at the cursor,
//! looks the callee up in the stdlib model, and renders one signature
//! per overload with the active signature and parameter selected from
//! the cursor's argument position.

use luck_semantic::stdlib_model::{StdlibArgKind, StdlibEntry, StdlibParam, StdlibSignature};
use tower_lsp::lsp_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, SignatureHelp,
    SignatureHelpParams, SignatureInformation,
};

use crate::backend::DocumentState;
use crate::providers::cursor::find_call_site_at;
use crate::stdlib_render::param_label;

#[must_use]
pub fn signature_help(doc: &DocumentState, params: &SignatureHelpParams) -> Option<SignatureHelp> {
    let position = params.text_document_position_params.position;
    let offset = doc.line_index.offset(&doc.text, position);
    let call = find_call_site_at(&doc.parsed.block, &doc.text, offset)?;
    // Semantic resolution instead of a textual lookup: shaped locals
    // (`f:read(` after `local f = io.open(...)`), literal receivers
    // (`("x"):rep(`), and shadowed bases all resolve or refuse
    // correctly. Method params exclude self, matching the
    // paren-relative argument count.
    let (name, resolved) = doc.semantic.resolve_callee(call.call)?;
    let StdlibEntry::Function(func) = resolved.entry else {
        return None;
    };
    let deprecated_message = func.deprecated.as_ref().map(|dep| dep.message.as_str());
    let signatures: Vec<SignatureInformation> = func
        .signatures
        .iter()
        .map(|sig| build_signature(&name, sig, deprecated_message, func.must_use))
        .collect();

    let active_signature = func.signature_index_for_active_param(call.active_param as usize);
    let active_sig = &func.signatures[active_signature];
    let active_param = clamp_active_param(call.active_param, active_sig);

    Some(SignatureHelp {
        signatures,
        active_signature: Some(active_signature as u32),
        active_parameter: Some(active_param),
    })
}

fn clamp_active_param(active: u32, sig: &StdlibSignature) -> u32 {
    let max = if sig.params.is_empty() {
        0
    } else {
        (sig.params.len() - 1) as u32
    };
    active.min(max)
}

fn build_signature(
    name: &str,
    sig: &StdlibSignature,
    deprecated_message: Option<&str>,
    must_use: bool,
) -> SignatureInformation {
    let mut label = format!("{name}(");
    let mut parameters: Vec<ParameterInformation> = Vec::new();
    for (idx, param) in sig.params.iter().enumerate() {
        if idx > 0 {
            label.push_str(", ");
        }
        let start = label.chars().count() as u32;
        label.push_str(&param_label(param));
        let end = label.chars().count() as u32;
        parameters.push(ParameterInformation {
            label: ParameterLabel::LabelOffsets([start, end]),
            documentation: param_doc(param),
        });
    }
    label.push(')');

    let mut documentation = String::new();
    if let Some(message) = deprecated_message {
        documentation.push_str(&format!("**Deprecated:** {message}\n"));
    }
    if must_use {
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

fn param_doc(param: &StdlibParam) -> Option<Documentation> {
    if let StdlibArgKind::Constant(values) = &param.kind {
        // Large generated sets (Roblox service and class names) would
        // swamp the panel; show a prefix and the total.
        const PREVIEW_LIMIT: usize = 8;
        let mut body = values
            .iter()
            .take(PREVIEW_LIMIT)
            .map(|constant| format!("`\"{}\"`", constant.value))
            .collect::<Vec<_>>()
            .join(", ");
        if values.len() > PREVIEW_LIMIT {
            body.push_str(&format!(", ... ({} values)", values.len()));
        }
        return Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("one of: {body}"),
        }));
    }
    if let Some(deprecation) = &param.deprecated {
        return Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("**Deprecated:** {}", deprecation.message),
        }));
    }
    if !param.required {
        return Some(Documentation::String("optional".to_string()));
    }
    None
}
