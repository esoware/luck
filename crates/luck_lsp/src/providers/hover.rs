//! The type of the name at the cursor as Luau prints it, and the comment a
//! definition file has above what it names.

use ls_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::document::DocumentState;
use crate::providers::cursor::name_at;
use crate::type_check::Types;

pub(crate) fn hover(
    document: &DocumentState,
    position: Position,
    types: &mut Types<'_>,
) -> Option<Hover> {
    // Only a name hovers, so the space inside a function doesn't hover the
    // whole function.
    let (_, name) = name_at(document, document.offset(position))?;
    let description = types.describe(name.span.start)?;
    let mut value = format!("```luau\n{}\n```", description.ty);
    if let Some(documentation) = types.documentation(&description.declaration) {
        value.push_str("\n\n");
        value.push_str(&documentation);
    }
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value }),
        range: Some(document.range(description.span)),
    })
}
