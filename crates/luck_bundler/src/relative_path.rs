use std::path::{self, Component, Path, PathBuf};

use luck_resolver::{FileSystem, to_slash_lossy};

/// `project_root` canonicalized, or made absolute when it doesn't exist.
///
/// An empty root, which `luck bundle main.luau` passes for the entry's parent,
/// is the working directory. When that is unavailable too, the root comes back
/// unchanged and the build goes on, though host paths can then reach the
/// output.
pub(crate) fn absolute_project_root(file_system: &impl FileSystem, project_root: &Path) -> PathBuf {
    let root = if project_root.as_os_str().is_empty() { Path::new(".") } else { project_root };
    file_system
        .canonicalize(root)
        .or_else(|_| path::absolute(root))
        .unwrap_or_else(|_| root.to_path_buf())
}

/// The project-relative form of `path`, which is what the bundle carries.
/// `root` comes from [`absolute_project_root`].
///
/// A module above the root gets `..` segments, so none of the names on the
/// root's side reach the output. When the two share no root at all, as with
/// another Windows drive or UNC share, only the file name is kept.
pub(crate) fn make_relative(path: &Path, root: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(root) {
        return to_slash_lossy(relative).into_owned();
    }

    let path_components: Vec<Component<'_>> = path.components().collect();
    let root_components: Vec<Component<'_>> = root.components().collect();
    let shared = path_components
        .iter()
        .zip(&root_components)
        .take_while(|(from_path, from_root)| from_path == from_root)
        .count();

    // A drive or share and the root after it have to match before there is
    // a shared root at all.
    let root_length = root_components
        .iter()
        .take_while(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
        .count();
    if shared == 0 || shared < root_length {
        return path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
    }

    let mut relative = "../".repeat(root_components.len() - shared);
    let rest: Vec<_> = path_components[shared..]
        .iter()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect();
    relative.push_str(&rest.join("/"));
    relative
}

#[cfg(test)]
mod tests {
    use std::env;

    use luck_resolver::FileSystemOs;

    use super::*;

    #[test]
    fn path_inside_the_project_root_loses_the_prefix() {
        assert_eq!(
            make_relative(Path::new("/proj/src/utils.luau"), Path::new("/proj")),
            "src/utils.luau"
        );
        // A trailing separator on the root must not survive into the path.
        assert_eq!(make_relative(Path::new("/proj/main.luau"), Path::new("/proj/")), "main.luau");
    }

    /// A vendored dependency above the root keeps its shape, but nothing from
    /// the root's own side of the tree is emitted.
    #[test]
    fn path_above_the_project_root_walks_up() {
        assert_eq!(
            make_relative(Path::new("/work/shared/util.luau"), Path::new("/work/proj/src")),
            "../../shared/util.luau"
        );
        assert_eq!(
            make_relative(Path::new("/work/sibling/a.luau"), Path::new("/work/proj")),
            "../sibling/a.luau"
        );
    }

    /// A path on another drive or share can't be made relative, and an
    /// absolute path must never reach the output.
    #[cfg(windows)]
    #[test]
    fn path_without_a_shared_root_keeps_only_the_file_name() {
        assert_eq!(
            make_relative(Path::new(r"E:\elsewhere\lib.luau"), Path::new(r"D:\proj")),
            "lib.luau"
        );
        assert_eq!(
            make_relative(Path::new(r"\\server-a\share\lib.luau"), Path::new(r"\\server-b\proj")),
            "lib.luau"
        );
        assert_eq!(
            make_relative(Path::new(r"\\server\other\lib.luau"), Path::new(r"\\server\share\proj")),
            "lib.luau"
        );
        assert_eq!(make_relative(Path::new(r"D:\lib.luau"), Path::new(r"\proj")), "lib.luau");
    }

    #[cfg(windows)]
    #[test]
    fn path_on_the_same_drive_walks_up() {
        assert_eq!(
            make_relative(Path::new(r"D:\work\shared\util.luau"), Path::new(r"D:\work\proj")),
            "../shared/util.luau"
        );
    }

    /// `luck bundle main.luau` passes the entry's parent, which is empty for a
    /// bare file name.
    #[test]
    fn empty_project_root_resolves_to_the_working_directory() {
        let working_directory =
            FileSystemOs.canonicalize(&env::current_dir().expect("cwd")).expect("canonicalize");
        assert_eq!(absolute_project_root(&FileSystemOs, Path::new("")), working_directory);
        assert_eq!(absolute_project_root(&FileSystemOs, Path::new(".")), working_directory);

        let root = absolute_project_root(&FileSystemOs, Path::new(""));
        assert_eq!(make_relative(&root.join("main.luau"), &root), "main.luau");
    }

    #[test]
    fn a_missing_project_root_is_made_absolute() {
        let root = absolute_project_root(&FileSystemOs, Path::new("missing/project"));
        assert!(root.is_absolute(), "{}", root.display());
        assert!(root.ends_with("missing/project"), "{}", root.display());
    }
}
