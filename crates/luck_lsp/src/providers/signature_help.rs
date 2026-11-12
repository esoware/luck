use luck_semantic::{StdlibArgKind, StdlibEntry, StdlibParam, StdlibSignature};
use luck_token::index_to_offset;
use tower_lsp_server::ls_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, Position,
    SignatureHelp, SignatureInformation,
};

use crate::document::DocumentState;
use crate::providers::cursor::{TextKind, find_call_site_at, text_kind_at};
use crate::providers::stdlib_render::{constant_preview, param_label};

pub(crate) fn signature_help(
    document: &DocumentState,
    position: Position,
) -> Option<SignatureHelp> {
    let offset = document.offset(position);
    if text_kind_at(document, offset) == TextKind::Comment {
        return None;
    }
    let site = find_call_site_at(document, offset)?;
    // Semantic resolution handles shaped locals (`f:read(` after
    // `local f = io.open(...)`) and literal receivers (`("x"):rep(`), and
    // gives a shadowed base no help. Method params exclude `self`, matching
    // the argument count inside the parens.
    let callee = document.semantic.resolve_callee(site.call)?;
    let StdlibEntry::Function(function) = callee.entry() else {
        return None;
    };
    let name = callee.name();
    let deprecated_message =
        function.deprecated.as_ref().map(|deprecation| deprecation.message.as_str());
    let signatures: Vec<SignatureInformation> = function
        .signatures
        .iter()
        .map(|signature| {
            build_signature(&name, signature, deprecated_message, function.is_must_use)
        })
        .collect();

    let active_signature = function.signature_index_for_active_param(site.active_param);
    let active_param_count = function.signatures.get(active_signature)?.params.len();
    let active_param = site.active_param.min(active_param_count.saturating_sub(1));

    Some(SignatureHelp {
        signatures,
        active_signature: Some(index_to_offset(active_signature)),
        active_parameter: Some(index_to_offset(active_param)),
    })
}

fn build_signature(
    name: &str,
    signature: &StdlibSignature,
    deprecated_message: Option<&str>,
    is_must_use: bool,
) -> SignatureInformation {
    let mut label = format!("{name}(");
    let mut params: Vec<ParameterInformation> = Vec::new();
    for (i, param) in signature.params.iter().enumerate() {
        if i > 0 {
            label.push_str(", ");
        }
        let start = index_to_offset(label.chars().count());
        label.push_str(&param_label(param));
        let end = index_to_offset(label.chars().count());
        params.push(ParameterInformation {
            label: ParameterLabel::LabelOffsets([start, end]),
            documentation: param_documentation(param),
        });
    }
    label.push(')');

    let mut documentation = deprecated_message
        .map_or_else(String::new, |message| format!("**Deprecated:** {message}\n"));
    if is_must_use {
        documentation.push_str("\n_Don't discard the return value._");
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
        parameters: Some(params),
        active_parameter: None,
    }
}

fn param_documentation(param: &StdlibParam) -> Option<Documentation> {
    if let StdlibArgKind::Constant(values) = &param.kind {
        let preview =
            constant_preview(values, ", ", |constant| format!("`\"{}\"`", constant.value));
        return Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("one of: {preview}"),
        }));
    }
    if let Some(deprecation) = &param.deprecated {
        return Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("**Deprecated:** {}", deprecation.message),
        }));
    }
    if !param.is_required {
        return Some(Documentation::String("optional".to_owned()));
    }
    None
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use luck_token::offset_to_index;
    use rustc_hash::FxHashSet;

    use super::*;

    fn active_signature(help: &SignatureHelp) -> &SignatureInformation {
        &help.signatures[offset_to_index(help.active_signature.unwrap_or(0))]
    }

    #[test]
    fn signature_help_resolves_shaped_local_method() {
        let text = "local f = io.open('x')\nf:setvbuf('full', 512)";
        let document =
            DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());
        // Cursor inside the second argument.
        let help =
            signature_help(&document, Position::new(1, 18)).expect("file method signature help");
        let signature = active_signature(&help);
        assert!(signature.label.starts_with("f:setvbuf("), "method rendering: {}", signature.label);
        assert_eq!(help.active_parameter, Some(1), "cursor is in arg 2");
    }

    #[test]
    fn signature_help_marks_method_active_param_excluding_self() {
        let text = "local p = game:GetService('Players')";
        let document =
            DocumentState::new(text.to_owned(), 1, LuaTarget::LuauRoblox, FxHashSet::default());
        let utf16_column = index_to_offset(text.find("'Players'").expect("literal") + 3);
        let help = signature_help(&document, Position::new(0, utf16_column))
            .expect("GetService signature help");
        let signature = active_signature(&help);
        assert!(signature.label.starts_with("game:GetService("), "label: {}", signature.label);
        // self is excluded: exactly one declared parameter, and the cursor
        // in the first paren argument selects it.
        assert_eq!(signature.parameters.as_ref().map(Vec::len), Some(1));
        assert_eq!(help.active_parameter, Some(0));
    }

    #[test]
    fn signature_help_refuses_shadowed_base() {
        let text = "local string = 1\nstring.format('x')";
        let document =
            DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());
        assert!(
            signature_help(&document, Position::new(1, 15)).is_none(),
            "shadowed base must not show stdlib signatures"
        );
    }

    #[test]
    fn signature_help_finds_a_call_at_stmt_position() {
        // Statement-position calls never pass through visit_expr;
        // the call hook must find them.
        let text = "collectgarbage('collect')";
        let document =
            DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());
        let help =
            signature_help(&document, Position::new(0, 16)).expect("statement-position call");
        assert!(
            help.signatures.iter().any(|signature| signature.label.starts_with("collectgarbage(")),
            "{help:?}"
        );
    }

    #[test]
    fn signature_help_ends_at_the_closing_paren() {
        let document =
            DocumentState::new("print(1)".to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());
        let help = signature_help(&document, Position::new(0, 8));
        assert!(help.is_none(), "the cursor is past `)`, outside the call: {help:?}");
    }

    #[test]
    fn signature_help_counts_commas_before_the_cursor() {
        let text = "string.rep(\"a\" , 3 )";
        let document =
            DocumentState::new(text.to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());
        let before_comma =
            signature_help(&document, Position::new(0, 15)).expect("inside the call");
        assert_eq!(before_comma.active_parameter, Some(0), "no comma before the cursor yet");
        let before_paren =
            signature_help(&document, Position::new(0, 19)).expect("inside the call");
        assert_eq!(before_paren.active_parameter, Some(1), "one comma before the cursor");
    }

    #[test]
    fn signature_help_stays_inside_an_unclosed_argument_list() {
        let document =
            DocumentState::new("print(1".to_owned(), 1, LuaTarget::Lua54, FxHashSet::default());
        assert!(
            signature_help(&document, Position::new(0, 7)).is_some(),
            "the closing paren hasn't been typed yet"
        );
    }

    #[test]
    fn signature_help_offers_nothing_inside_a_comment_argument() {
        let document = DocumentState::new(
            "print(--[[ note ]] 1)".to_owned(),
            1,
            LuaTarget::Lua54,
            FxHashSet::default(),
        );
        assert!(signature_help(&document, Position::new(0, 13)).is_none());
    }
}
