use std::iter;

use ls_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, Position,
    SignatureHelp, SignatureInformation,
};
use luck_ast::{FunctionArgs, GetSpan as _};
use luck_stdlib::{StdlibArgKind, StdlibEntry, StdlibParam, StdlibSignature};
use luck_token::TokenKind;

use crate::document::DocumentState;
use crate::providers::cursor::{TextKind, text_kind_at};
use crate::providers::stdlib_render::{constant_preview, param_label};

pub(crate) fn signature_help(
    document: &DocumentState,
    position: Position,
) -> Option<SignatureHelp> {
    let offset = document.offset(position);
    if text_kind_at(document, offset) == TextKind::Comment {
        return None;
    }
    // The innermost call whose parentheses hold the cursor, found on the way
    // up from the node there. An unclosed list ends where the parser
    // stopped, so its end still counts as inside it, but past a real `)` the
    // cursor has left the call. `f"..."` and `f{...}` have no parentheses to
    // be inside.
    let source = document.text().as_bytes();
    let nodes = document.nodes();
    let innermost = nodes.node_at(offset)?;
    let (call, args) =
        iter::once(innermost).chain(nodes.ancestor_ids(innermost)).find_map(|id| {
            nodes.kind(id).as_suffixed_expr()?.calls().find_map(|(_, call)| {
                let FunctionArgs::Parenthesized { span: parens, args } = call.args else {
                    return None;
                };
                let is_closed = source[(parens.end - 1) as usize] == b')';
                let is_inside = parens.start < offset
                    && offset <= parens.end
                    && !(is_closed && offset == parens.end);
                is_inside.then_some((call, args))
            })
        })?;
    // Reading the token after each argument, rather than counting commas,
    // can't mistake one inside a nested argument for a separator of this
    // list.
    let tokens = document.tokens();
    let active_param = args
        .iter()
        .filter(|arg| {
            let next = tokens.partition_point(|token| token.span.start < arg.span().end);
            tokens
                .get(next)
                .is_some_and(|token| token.kind == TokenKind::Comma && token.span.end <= offset)
        })
        .count();
    // Semantic resolution handles shaped locals (`g:GetService(` after
    // `local g = game`) and literal receivers (`("x"):rep(`), and
    // gives a shadowed base no help. Method params exclude `self`, matching
    // the argument count inside the parens.
    let callee = document.semantic().resolve_callee(call)?;
    let StdlibEntry::Function(function) = callee.entry() else {
        return None;
    };
    let name = callee.name();
    let deprecation = function.deprecated.as_ref().map(|deprecation| match deprecation.advice() {
        Some(advice) => format!("**Deprecated:** {advice}\n"),
        None => "**Deprecated**\n".to_owned(),
    });
    let signatures: Vec<SignatureInformation> = function
        .signatures
        .iter()
        .map(|signature| build_signature(&name, signature, deprecation.clone()))
        .collect();

    let active_signature = function.signature_index_for_active_param(active_param);
    let active_param_count = function.signatures[active_signature].params.len();

    Some(SignatureHelp {
        signatures,
        active_signature: Some(
            u32::try_from(active_signature)
                .expect("a function has fewer signatures than `u32::MAX`"),
        ),
        active_parameter: Some(
            u32::try_from(active_param.min(active_param_count.saturating_sub(1)))
                .expect("a signature has fewer parameters than `u32::MAX`"),
        ),
    })
}

fn build_signature(
    name: &str,
    signature: &StdlibSignature<'_>,
    deprecation: Option<String>,
) -> SignatureInformation {
    let mut label = format!("{name}(");
    let mut params: Vec<ParameterInformation> = Vec::new();
    for (i, param) in signature.params.iter().enumerate() {
        if i > 0 {
            label.push_str(", ");
        }
        let start = u32::try_from(label.chars().count())
            .expect("a signature label is shorter than `u32::MAX` characters");
        label.push_str(&param_label(param));
        let end = u32::try_from(label.chars().count())
            .expect("a signature label is shorter than `u32::MAX` characters");
        params.push(ParameterInformation {
            label: ParameterLabel::LabelOffsets([start, end]),
            documentation: param_documentation(param),
        });
    }
    label.push(')');

    SignatureInformation {
        label,
        documentation: deprecation.map(|value| {
            Documentation::MarkupContent(MarkupContent { kind: MarkupKind::Markdown, value })
        }),
        parameters: Some(params),
        active_parameter: None,
    }
}

fn param_documentation(param: &StdlibParam<'_>) -> Option<Documentation> {
    if let StdlibArgKind::Constant(values) = &param.kind {
        let preview = constant_preview(values, ", ", |constant| format!("`\"{constant}\"`"));
        return Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("one of: {preview}"),
        }));
    }
    if !param.is_required {
        return Some(Documentation::String("optional".to_owned()));
    }
    None
}

#[cfg(test)]
mod tests {
    use luck_token::index_to_offset;

    use super::*;
    use crate::fixtures::{document, roblox_document};

    fn active_signature(help: &SignatureHelp) -> &SignatureInformation {
        &help.signatures[help.active_signature.unwrap_or(0) as usize]
    }

    #[test]
    fn signature_help_resolves_shaped_local_method() {
        let text = "\
local g = game
g:FindFirstChild('x', true)";
        let document = roblox_document(text);
        // Cursor inside the second argument.
        let help = signature_help(&document, Position::new(1, 23))
            .expect("instance method signature help");
        let signature = active_signature(&help);
        assert!(
            signature.label.starts_with("g:FindFirstChild("),
            "method rendering: {}",
            signature.label
        );
        assert_eq!(help.active_parameter, Some(1), "cursor is in arg 2");
    }

    #[test]
    fn signature_help_marks_method_active_param_excluding_self() {
        let text = "local p = game:GetService('Players')";
        let document = roblox_document(text);
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
        let text = "\
local string = 1
string.format('x')";
        let document = document(text);
        assert!(
            signature_help(&document, Position::new(1, 15)).is_none(),
            "shadowed base must not show stdlib signatures"
        );
    }

    #[test]
    fn signature_help_finds_a_call_at_stmt_position() {
        let text = "print('collect')";
        let document = document(text);
        let help = signature_help(&document, Position::new(0, 7)).expect("statement-position call");
        assert!(
            help.signatures.iter().any(|signature| signature.label.starts_with("print(")),
            "{help:?}"
        );
    }

    #[test]
    fn signature_help_ends_at_the_closing_paren() {
        let document = document("print(1)");
        let help = signature_help(&document, Position::new(0, 8));
        assert!(help.is_none(), "the cursor is past `)`, outside the call: {help:?}");
    }

    #[test]
    fn signature_help_counts_commas_before_the_cursor() {
        let text = "string.rep(\"a\" , 3 )";
        let document = document(text);
        let before_comma =
            signature_help(&document, Position::new(0, 15)).expect("inside the call");
        assert_eq!(before_comma.active_parameter, Some(0), "no comma before the cursor yet");
        let before_paren =
            signature_help(&document, Position::new(0, 19)).expect("inside the call");
        assert_eq!(before_paren.active_parameter, Some(1), "one comma before the cursor");
    }

    #[test]
    fn signature_help_reads_the_innermost_call_around_the_cursor() {
        let document = document("print(string.rep(\"a\", 2), 3)");
        let label_at = |utf16_column| {
            let help = signature_help(&document, Position::new(0, utf16_column))?;
            Some((active_signature(&help).label.clone(), help.active_parameter))
        };
        let (inner, inner_param) = label_at(23).expect("inside `string.rep`");
        assert!(inner.starts_with("string.rep("), "{inner}");
        assert_eq!(inner_param, Some(1));
        let (outer, outer_param) = label_at(27).expect("after `string.rep(...)`");
        assert!(outer.starts_with("print("), "{outer}");
        assert_eq!(outer_param, Some(0), "`print` takes its arguments as one vararg");
    }

    #[test]
    fn signature_help_stays_inside_an_unclosed_argument_list() {
        let document = document("print(1");
        assert!(
            signature_help(&document, Position::new(0, 7)).is_some(),
            "the closing paren hasn't been typed yet"
        );
    }

    #[test]
    fn signature_help_offers_nothing_inside_a_comment_argument() {
        let document = document("print(--[[ note ]] 1)");
        assert!(signature_help(&document, Position::new(0, 13)).is_none());
    }
}
