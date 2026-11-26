//! Where the server reads every source: the disk, or, once the client gives a
//! config, the files the client holds, the documents it opens and the files
//! it pushes, as an editor with nothing on disk does.
//!
//! A client's file goes by a path made of its URI, `/scheme/authority/path`,
//! as `inmemory://model/1` is `/inmemory/model/1`, so the resolver, the
//! project's globs, and Luau's checker read it as they read a file on disk,
//! and each path the server hands back goes by the URI the file came with.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{io, iter};

use ls_types::Uri;
use luck_resolver::{DirEntry, FileMetadata, FileSystem, FileSystemOs, lexically_normalized};
use parking_lot::RwLock;

#[derive(Debug, Clone)]
pub(crate) enum Files {
    Disk,
    Client(Arc<ClientFiles>),
}

/// The files a client holds, by the path its URI makes.
#[derive(Debug, Default)]
pub(crate) struct ClientFiles {
    files: RwLock<BTreeMap<PathBuf, ClientFile>>,
}

#[derive(Debug)]
struct ClientFile {
    uri: Uri,
    /// What the client last pushed, which a module reads unless it's open, as
    /// a file on disk is.
    pushed: Option<Arc<str>>,
    /// The text while the client has it open.
    open: Option<Arc<str>>,
}

impl Files {
    /// The path of the file `uri` names: on disk, a `file:` URI's, and of the
    /// client's, any URI's.
    pub(crate) fn path(&self, uri: &Uri) -> Option<PathBuf> {
        match self {
            Self::Disk => file_path(uri),
            Self::Client(_) => Some(client_path(uri)),
        }
    }

    /// The URI of the file at `path`, which [`Self::path`] gave.
    pub(crate) fn uri(&self, path: &Path) -> Option<Uri> {
        match self {
            Self::Disk => Uri::from_file_path(path),
            Self::Client(client) => client.files.read().get(path).map(|file| file.uri.clone()),
        }
    }

    pub(crate) fn client(&self) -> Option<&ClientFiles> {
        match self {
            Self::Disk => None,
            Self::Client(client) => Some(client),
        }
    }

    fn file_system(&self) -> &dyn FileSystem {
        match self {
            Self::Disk => &FileSystemOs,
            Self::Client(client) => &**client,
        }
    }
}

impl FileSystem for Files {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.file_system().read(path)
    }

    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        self.file_system().metadata(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.file_system().read_dir(path)
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.file_system().canonicalize(path)
    }

    fn canonicalize_existing(&self, path: &Path) -> PathBuf {
        self.file_system().canonicalize_existing(path)
    }
}

impl ClientFiles {
    /// Makes `text` the file at `path`, which `uri` names, or with `None`,
    /// deletes it, and says whether that created or deleted it.
    pub(crate) fn push(&self, path: &Path, uri: Uri, text: Option<Arc<str>>) -> bool {
        self.set(path, uri, |file| file.pushed = text)
    }

    /// Makes `text`, or with `None`, nothing, what the client has open at
    /// `path`, which `uri` names, and says whether that created or deleted
    /// the file.
    pub(crate) fn open(&self, path: &Path, uri: Uri, text: Option<Arc<str>>) -> bool {
        self.set(path, uri, |file| file.open = text)
    }

    fn set(&self, path: &Path, uri: Uri, change: impl FnOnce(&mut ClientFile)) -> bool {
        let mut files = self.files.write();
        let was_there = files.contains_key(path);
        let file =
            files.entry(path.to_path_buf()).or_insert(ClientFile { uri, pushed: None, open: None });
        change(file);
        let is_there = file.pushed.is_some() || file.open.is_some();
        if !is_there {
            files.remove(path);
        }
        was_there != is_there
    }

    /// Every file's path, in order.
    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        self.files.read().keys().cloned().collect()
    }

    /// Whether a file lies under `path`, which is then a directory.
    fn has_below(&self, path: &Path) -> bool {
        let files = self.files.read();
        let mut after = files.range::<Path, _>((Bound::Excluded(path), Bound::Unbounded));
        after.next().is_some_and(|(file, _)| file.starts_with(path))
    }
}

impl FileSystem for ClientFiles {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let files = self.files.read();
        let file = files.get(path).ok_or(io::ErrorKind::NotFound)?;
        let text = file.pushed.as_ref().or(file.open.as_ref()).ok_or(io::ErrorKind::NotFound)?;
        Ok(text.as_bytes().to_vec())
    }

    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        let is_file = self.files.read().contains_key(path);
        let is_dir = self.has_below(path);
        if !is_file && !is_dir {
            return Err(io::ErrorKind::NotFound.into());
        }
        Ok(FileMetadata { is_file, is_dir })
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let files = self.files.read();
        let below = files
            .range::<Path, _>((Bound::Excluded(path), Bound::Unbounded))
            .map_while(|(file, _)| file.strip_prefix(path).ok());
        let mut entries: Vec<DirEntry> = Vec::new();
        for relative in below {
            let mut components = relative.iter();
            let Some(name) = components.next() else {
                continue;
            };
            if entries.last().is_some_and(|last| last.name == name) {
                continue;
            }
            let is_file = components.next().is_none();
            let metadata = FileMetadata { is_file, is_dir: !is_file };
            entries.push(DirEntry { name: name.to_owned(), metadata: Some(metadata) });
        }
        if entries.is_empty() {
            return Err(io::ErrorKind::NotFound.into());
        }
        Ok(entries)
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        let normalized = lexically_normalized(path);
        self.metadata(&normalized)?;
        Ok(normalized)
    }

    /// A client's paths are whole as its URIs make them, with no link to
    /// follow and no working directory to start from.
    fn canonicalize_existing(&self, path: &Path) -> PathBuf {
        lexically_normalized(path)
    }
}

/// The path a `file:` URI names. `Uri::to_file_path` ignores the scheme, so
/// it would turn an `untitled:` buffer into a relative path.
fn file_path(uri: &Uri) -> Option<PathBuf> {
    if !uri.scheme().as_str().eq_ignore_ascii_case("file") {
        return None;
    }
    uri.to_file_path().map(Cow::into_owned)
}

/// `/scheme/authority/path`, the path of the client's file at `uri`.
fn client_path(uri: &Uri) -> PathBuf {
    let authority = uri.authority().map(|authority| authority.as_str()).unwrap_or_default();
    let path = uri.path().decode().to_string_lossy();
    let parts = [uri.scheme().as_str(), authority, &path];
    let joined: String =
        iter::once("/").chain(parts.iter().flat_map(|part| [*part, "/"])).collect();
    lexically_normalized(Path::new(&joined))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use super::*;

    fn uri(text: &str) -> Uri {
        Uri::from_str(text).expect("valid URI")
    }

    #[test]
    fn only_a_file_uri_has_a_path_on_disk() {
        assert_eq!(Files::Disk.path(&uri("untitled:Untitled-1")), None);
        let saved = Files::Disk.path(&uri("FILE:///tmp/main.luau"));
        assert!(saved.is_some_and(|path| path.ends_with("main.luau")));
    }

    #[test]
    fn a_client_uri_is_a_path_of_its_scheme_authority_and_path() {
        let files = Files::Client(Arc::default());
        let path = |text: &str| files.path(&uri(text)).expect("every URI has a client path");
        assert_eq!(
            path("inmemory://model/1"),
            lexically_normalized(Path::new("/inmemory/model/1"))
        );
        assert_eq!(
            path("inmemory://model/src/main.luau"),
            lexically_normalized(Path::new("/inmemory/model/src/main.luau"))
        );
        assert_eq!(
            path("untitled:Untitled-1"),
            lexically_normalized(Path::new("/untitled/Untitled-1"))
        );
    }

    #[test]
    fn a_client_s_files_list_as_directories_do() {
        let client = ClientFiles::default();
        let push = |name: &str| {
            let path = lexically_normalized(Path::new(name));
            client.push(&path, uri("inmemory://model/x"), Some(Arc::from("return {}")))
        };
        assert!(push("/inmemory/model/src/main.luau"));
        assert!(push("/inmemory/model/src/util/init.luau"));
        assert!(push("/inmemory/model/src.luau"));
        assert!(!push("/inmemory/model/src.luau"), "pushing it again creates nothing");

        let source = lexically_normalized(Path::new("/inmemory/model/src"));
        let listed: Vec<(String, bool)> = client
            .read_dir(&source)
            .expect("a directory")
            .into_iter()
            .map(|entry| {
                (entry.name.to_string_lossy().into_owned(), entry.metadata.unwrap().is_dir)
            })
            .collect();
        assert_eq!(listed, [("main.luau".to_owned(), false), ("util".to_owned(), true)]);
        assert!(client.metadata(&source).unwrap().is_dir);
        assert!(client.metadata(&source.join("main.luau")).unwrap().is_file);
        assert!(client.metadata(&source.join("gone.luau")).is_err());

        let main = source.join("main.luau");
        assert!(client.push(&main, uri("inmemory://model/src/main.luau"), None));
        assert!(client.read(&main).is_err(), "a deleted file is gone");
    }
}
