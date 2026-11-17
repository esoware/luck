use alloc::sync::Arc;

use luck_allocator::Allocator;
use luck_core::LuaTarget;

use crate::document::{DocumentSettings, DocumentState};
use crate::settings::ProjectSettings;

/// `text` as an untitled document under the default settings.
pub(crate) fn document(text: &str, target: LuaTarget) -> DocumentState {
    document_under(text, target, ProjectSettings::default())
}

pub(crate) fn document_under(
    text: &str,
    target: LuaTarget,
    project: ProjectSettings,
) -> DocumentState {
    let settings =
        DocumentSettings { path: None, project: Arc::new(project), target, is_included: true };
    DocumentState::new(text, 1, Arc::new(settings), Allocator::new())
}
