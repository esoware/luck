use alloc::sync::Arc;
use std::path::PathBuf;

use luck_config::Project;
use luck_core::LuaTarget;
use luck_formatter::FormatOptions;
use luck_linter::LintConfig;

use crate::analyzed::Arena;
use crate::document::DocumentState;
use crate::line_index::{LineIndex, PositionEncoding};
use crate::settings::{DocumentSettings, LoadedProject};

/// `text` as an untitled document with no `luck.json`, in UTF-16 positions,
/// which a client that names no encoding gets.
pub(crate) fn document(text: &str, target: LuaTarget) -> DocumentState {
    document_in(text, target, Project::defaults(PathBuf::from(".")))
}

/// `text` as an untitled document under `project`'s settings.
pub(crate) fn document_in(text: &str, target: LuaTarget, project: Project) -> DocumentState {
    let lint = project.lint_for(None);
    document_with(text, target, project, lint)
}

/// `text` as an untitled document linted under `lint`.
pub(crate) fn document_linted_by(text: &str, target: LuaTarget, lint: LintConfig) -> DocumentState {
    document_with(text, target, Project::defaults(PathBuf::from(".")), Arc::new(lint))
}

fn document_with(
    text: &str,
    target: LuaTarget,
    project: Project,
    lint: Arc<LintConfig>,
) -> DocumentState {
    let settings = DocumentSettings {
        project: Arc::new(LoadedProject::new(project)),
        target,
        lint,
        format: FormatOptions::default(),
        is_included: true,
    };
    DocumentState::new(
        Arc::from(text),
        Arc::new(settings),
        Arc::new(LineIndex::new(text, PositionEncoding::Utf16)),
        Arena::from_spare(&Arc::default()),
    )
}
