//! The type of the name at the cursor as Luau prints it, and the comment a
//! definition file has above what it names.

use ls_types::{Hover, HoverContents, MarkupContent, MarkupKind};

use crate::document::DocumentState;
use crate::type_check::Types;

/// The hover of the name that starts at `name_start`, which
/// [`name_start`](crate::providers::cursor::name_start) finds.
pub(crate) fn hover(
    document: &DocumentState,
    name_start: u32,
    types: &mut Types<'_>,
) -> Option<Hover> {
    let description = types.describe(name_start)?;
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
