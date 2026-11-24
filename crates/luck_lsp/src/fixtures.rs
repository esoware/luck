use alloc::sync::Arc;
use core::num::NonZero;
use std::path::{Path, PathBuf};

use crossbeam_channel::unbounded;
use luck_config::Project;
use luck_formatter::FormatOptions;
use luck_linter::{LintConfig, Linter};
use luck_resolver::{FileSystem as _, FileSystemOs};
use luck_stdlib::Stdlib;
use luck_token::LuaTarget;

use crate::analyzed::Arena;
use crate::document::DocumentState;
use crate::line_index::{LineIndex, PositionEncoding};
use crate::session::Session;
use crate::settings::{DocumentSettings, LoadedProject};

/// `text` as an untitled document with no `luck.json`, in UTF-16 positions,
/// which a client that names no encoding gets.
pub(crate) fn document(text: &str, target: LuaTarget) -> DocumentState {
    document_in(text, target, Project::defaults(PathBuf::from(".")))
}

/// `text` as an untitled document under `project`'s settings.
pub(crate) fn document_in(text: &str, target: LuaTarget, project: Project) -> DocumentState {
    let linter = project.linter_for(None);
    document_with(text, target, project, linter, None)
}

/// `text` as the document of the file at `path` under `project`'s settings.
pub(crate) fn document_at(
    text: &str,
    target: LuaTarget,
    project: Project,
    path: &Path,
) -> DocumentState {
    let linter = project.linter_for(Some(path));
    document_with(text, target, project, linter, Some(FileSystemOs.canonicalize_existing(path)))
}

/// `text` as an untitled document linted under `lint`.
pub(crate) fn document_linted_by(text: &str, target: LuaTarget, lint: LintConfig) -> DocumentState {
    let linter = Arc::new(Linter::new(lint));
    document_with(text, target, Project::defaults(PathBuf::from(".")), linter, None)
}

/// A session with no workspace folders, in UTF-16 positions, whose tasks
/// nothing reads.
pub(crate) fn session() -> Session {
    Session::new(PositionEncoding::Utf16, unbounded().0, NonZero::<usize>::MIN)
}

fn document_with(
    text: &str,
    target: LuaTarget,
    project: Project,
    linter: Arc<Linter>,
    path: Option<PathBuf>,
) -> DocumentState {
    let settings = DocumentSettings {
        path,
        project: Arc::new(LoadedProject::new(project)),
        stdlib: Stdlib::new(target),
        linter,
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
