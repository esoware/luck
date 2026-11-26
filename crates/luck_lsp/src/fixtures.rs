use std::fs;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::unbounded;
use ls_types::Uri;
use luck_config::{CONFIG_FILE_NAME, Project};
use luck_linter::{LintConfig, Linter};
use luck_resolver::{FileSystem as _, FileSystemOs, ResolverGeneric};

use crate::analyzed::Arena;
use crate::document::DocumentState;
use crate::files::Files;
use crate::line_index::{LineIndex, PositionEncoding};
use crate::session::Session;
use crate::settings::{DocumentSettings, LoadedProject};

/// `text` as an untitled document with no `luck.json`, in UTF-16 positions,
/// which a client that names no encoding gets.
pub(crate) fn document(text: &str) -> DocumentState {
    untitled(text, Project::defaults(PathBuf::from("."), FileSystemOs))
}

/// [`document`], in a project that loads Roblox's API.
pub(crate) fn roblox_document(text: &str) -> DocumentState {
    let directory = tempfile::tempdir().expect("tempdir");
    let config_path = directory.path().join(CONFIG_FILE_NAME);
    fs::write(&config_path, r#"{ "definitions": ["@roblox"] }"#).expect("write luck.json");
    untitled(text, Project::load(&config_path, FileSystemOs).expect("the config loads"))
}

/// `text` as the document of the file at `path` under `project`'s settings.
pub(crate) fn document_at(text: &str, project: Project, path: &Path) -> DocumentState {
    let linter = project.linter_for(Some(path));
    document_with(text, project, linter, Some(path))
}

/// `text` as an untitled document linted under `lint`.
pub(crate) fn document_linted_by(text: &str, lint: LintConfig) -> DocumentState {
    let linter = Arc::new(Linter::new(lint));
    document_with(text, Project::defaults(PathBuf::from("."), FileSystemOs), linter, None)
}

/// The URI the tests open their documents at.
pub(crate) fn uri() -> Uri {
    "file:///main.luau".parse().expect("valid URI")
}

/// A session with no workspace folders, in UTF-16 positions, whose tasks
/// nothing reads.
pub(crate) fn session() -> Session {
    let files = Files::new(None);
    Session::new(PositionEncoding::Utf16, unbounded().0, NonZero::<usize>::MIN, files, None)
}

fn untitled(text: &str, project: Project) -> DocumentState {
    let linter = project.linter_for(None);
    document_with(text, project, linter, None)
}

fn document_with(
    text: &str,
    project: Project,
    linter: Arc<Linter>,
    path: Option<&Path>,
) -> DocumentState {
    let stdlib = project.stdlib().clone();
    let files = Files::new(None);
    let resolver = Arc::new(ResolverGeneric::new_with_file_system(files.clone()));
    let settings = DocumentSettings {
        path: path.map(|located| FileSystemOs.canonicalize_existing(located)),
        located: path.map(Path::to_path_buf),
        project: Arc::new(LoadedProject::new(project, files, resolver)),
        linter,
        is_included: true,
        is_definitions: false,
    };
    DocumentState::new(
        Arc::from(text),
        Arc::new(settings),
        Arc::new(LineIndex::new(text, PositionEncoding::Utf16)),
        Arena::from_spare(&Arc::default()),
        stdlib,
    )
}
