use alloc::sync::Arc;
use std::path::PathBuf;

use luck_allocator::Allocator;
use luck_config::Project;
use luck_core::LuaTarget;
use luck_formatter::FormatOptions;
use luck_linter::LintConfig;

use crate::document::{DocumentSettings, DocumentState};

/// `text` as an untitled document with no `luck.json`.
pub(crate) fn document(text: &str, target: LuaTarget) -> DocumentState {
    document_in(text, target, Project::defaults(PathBuf::from(".")))
}

/// `text` as an untitled document under `project`'s settings.
pub(crate) fn document_in(text: &str, target: LuaTarget, project: Project) -> DocumentState {
    let lint = project.lint_for(None);
    document_with(text, target, Arc::new(project), lint)
}

/// `text` as an untitled document linted under `lint`.
pub(crate) fn document_linted_by(text: &str, target: LuaTarget, lint: LintConfig) -> DocumentState {
    let project = Arc::new(Project::defaults(PathBuf::from(".")));
    document_with(text, target, project, Arc::new(lint))
}

fn document_with(
    text: &str,
    target: LuaTarget,
    project: Arc<Project>,
    lint: Arc<LintConfig>,
) -> DocumentState {
    let settings = DocumentSettings {
        project,
        target,
        lint,
        format: FormatOptions::default(),
        is_included: true,
    };
    DocumentState::new([text], 1, Arc::new(settings), Allocator::new())
}
