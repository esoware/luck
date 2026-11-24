use ls_types::notification::{Notification as _, ShowMessage};
use ls_types::{MessageType, Range, ShowMessageParams, TextEdit};
use lsp_server::Notification;
use luck_allocator::Allocator;

use crate::document::DocumentState;
use crate::server::Task;
use crate::session::Session;

/// The document formatted, or with `range`, the statements it touches, as
/// one whole-document edit. `None` when `range` doesn't fit it, or it doesn't
/// parse, which also shows a warning.
pub(crate) fn formatting(
    session: &Session,
    document: &DocumentState,
    range: Option<Range>,
) -> Option<Vec<TextEdit>> {
    let byte_range = match range {
        Some(requested) => Some(document.byte_range(requested)?),
        None => None,
    };
    let action = if byte_range.is_some() { "range-format" } else { "format" };
    let options = &document.settings().format;
    // A scratch arena, so the formatting is freed as soon as it's done.
    let allocator = Allocator::new();
    let parse = document.parse();
    // `format_range` returns the whole file with text outside the range kept
    // byte-for-byte, so a whole-file edit only changes the range.
    let result = byte_range.map_or_else(
        || luck_formatter::format_parsed(&allocator, parse, options),
        |bytes| luck_formatter::format_range(&allocator, parse, options, bytes),
    );
    if let Some(first) = result.errors.first() {
        let message = format!("luck: can't {action} file (parse error): {}", first.message);
        let warning = ShowMessageParams { typ: MessageType::WARNING, message };
        session.send(Task::Notify(Notification::new(ShowMessage::METHOD.to_owned(), warning)));
        return None;
    }
    if result.output == document.text().as_bytes() {
        return Some(Vec::new());
    }
    // A text that's UTF-8 formats as UTF-8.
    let new_text = String::from_utf8_lossy(&result.output).into_owned();
    Some(vec![TextEdit { range: document.full_range(), new_text }])
}
