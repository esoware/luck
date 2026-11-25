use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::unbounded;
use ls_types::Uri;
use luck_config::Project;
use luck_formatter::FormatOptions;
use luck_linter::{LintConfig, Linter};
use luck_resolver::{FileSystem as _, FileSystemOs};
use luck_stdlib::Stdlib;
use luck_token::Variant;

use crate::analyzed::Arena;
use crate::document::DocumentState;
use crate::line_index::{LineIndex, PositionEncoding};
use crate::session::Session;
use crate::settings::{DocumentSettings, LoadedProject};

/// `text` as an untitled document with no `luck.json`, in UTF-16 positions,
/// which a client that names no encoding gets.
pub(crate) fn document(text: &str) -> DocumentState {
    untitled(text, Variant::Standalone)
}

/// [`document`], against Roblox's standard library.
pub(crate) fn roblox_document(text: &str) -> DocumentState {
    untitled(text, Variant::Roblox)
}

/// `text` as the document of the file at `path` under `project`'s settings.
pub(crate) fn document_at(text: &str, project: Project, path: &Path) -> DocumentState {
    let linter = project.linter_for(Some(path));
    let stdlib = project.stdlib().clone();
    document_with(text, stdlib, project, linter, Some(FileSystemOs.canonicalize_existing(path)))
}

/// `text` as an untitled document linted under `lint`.
pub(crate) fn document_linted_by(text: &str, lint: LintConfig) -> DocumentState {
    let linter = Arc::new(Linter::new(lint));
    let project = Project::defaults(PathBuf::from("."));
    document_with(text, Stdlib::new(Variant::Standalone), project, linter, None)
}

/// The URI the tests open their documents at.
pub(crate) fn uri() -> Uri {
    "file:///main.lua".parse().expect("valid URI")
}

/// A session with no workspace folders, in UTF-16 positions, whose tasks
/// nothing reads.
pub(crate) fn session() -> Session {
    Session::new(PositionEncoding::Utf16, unbounded().0, NonZero::<usize>::MIN)
}

fn untitled(text: &str, variant: Variant) -> DocumentState {
    let project = Project::defaults(PathBuf::from("."));
    let linter = project.linter_for(None);
    document_with(text, Stdlib::new(variant), project, linter, None)
}

fn document_with(
    text: &str,
    stdlib: Stdlib,
    project: Project,
    linter: Arc<Linter>,
    path: Option<PathBuf>,
) -> DocumentState {
    let settings = DocumentSettings {
        path,
        project: Arc::new(LoadedProject::new(project)),
        stdlib,
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
