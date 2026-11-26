//! The innermost call around the cursor, with each signature of its
//! callee's type as Luau prints a named function, and the comment a
//! definition file has above the callee.

use std::iter;

use ls_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, Position,
    SignatureHelp, SignatureInformation,
};
use luck_ast::{FunctionArgs, GetSpan as _};
use luck_token::{Span, TokenKind, index_to_offset, offset_to_index};

use crate::document::{DocumentState, span_text};
use crate::line_index::LineIndex;
use crate::providers::cursor::{TextKind, text_kind_at};
use crate::type_check::Types;

pub(crate) fn signature_help(
    document: &DocumentState,
    position: Position,
    types: &mut Types<'_>,
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
    // The callee as written, and a place in it that Luau reads as the whole
    // callee: the method's name, or else the callee's last byte.
    let callee = call.callee.span();
    let (name, callee_end) = match call.method {
        Some(method) => {
            (format!("{}:{}", span_text(document.text(), callee), method.text), method.span.start)
        }
        None => (span_text(document.text(), callee).to_owned(), callee.end.saturating_sub(1)),
    };
    let description = types.describe(callee_end)?;
    let documentation = types.documentation(&description.declaration).map(|value| {
        Documentation::MarkupContent(MarkupContent { kind: MarkupKind::Markdown, value })
    });
    let encoding = document.encoding();
    let mut fits = Vec::new();
    let signatures: Vec<SignatureInformation> = description
        .signatures
        .into_iter()
        .map(|signature| {
            // A trailing vararg takes every argument from its own on.
            let is_variadic = signature.params.last().is_some_and(|param| {
                signature.label[offset_to_index(param.start)..].starts_with("...")
            });
            let count = signature.params.len();
            fits.push(
                (active_param < count || is_variadic)
                    .then(|| if is_variadic { active_param.min(count - 1) } else { active_param }),
            );
            let label = format!("{name}{}", signature.label);
            let units = LineIndex::new(&label, encoding);
            let shift = index_to_offset(name.len());
            let parameters = signature
                .params
                .iter()
                .map(|param| ParameterInformation {
                    label: ParameterLabel::LabelOffsets([
                        units.length(Span::new(0, shift + param.start)),
                        units.length(Span::new(0, shift + param.end)),
                    ]),
                    documentation: None,
                })
                .collect();
            SignatureInformation {
                label,
                documentation: documentation.clone(),
                parameters: Some(parameters),
                active_parameter: None,
            }
        })
        .collect();
    // The first overload that takes as many arguments as there are so far.
    let (active_signature, active_parameter) = fits
        .iter()
        .enumerate()
        .find_map(|(index, parameter)| Some((index, (*parameter)?)))
        .unwrap_or((0, active_param));
    Some(SignatureHelp {
        active_parameter: Some(u32::try_from(active_parameter).unwrap_or(u32::MAX)),
        active_signature: Some(u32::try_from(active_signature).unwrap_or(u32::MAX)),
        signatures,
    })
}
