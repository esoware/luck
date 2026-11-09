//! The project-relative paths a bundle carries in place of host paths.

use std::env;
use std::path::Path;

use luck_core::normalize_path;

/// The project root as a normalized absolute path.
///
/// An empty or relative root resolves against the working directory. If that's
/// unavailable too, the input comes back unchanged and the build goes on, even
/// though host paths can then reach the output.
pub(crate) fn absolute_project_root(project_root: &Path) -> String {
    let normalized = normalize_path(project_root);
    if Path::new(&normalized).is_absolute() {
        return normalized;
    }
    match env::current_dir() {
        Ok(current_directory) => {
            let working_directory = normalize_path(&current_directory);
            if normalized.is_empty() || normalized == "." {
                working_directory
            } else {
                format!("{}/{normalized}", working_directory.trim_end_matches('/'))
            }
        }
        Err(_) => normalized,
    }
}

/// The project-relative form of `path`, which is what the bundle carries.
/// `root` comes from [`absolute_project_root`].
///
/// A module above the root gets `..` segments, so none of the names on the
/// root's side reach the output. When the two share no root at all, as with
/// another Windows drive or a UNC share, only the file name is kept.
pub(crate) fn make_relative(path: &str, root: &str) -> String {
    let root_directory = root.trim_end_matches('/');
    if let Some(relative) =
        path.strip_prefix(root_directory).and_then(|rest| rest.strip_prefix('/'))
    {
        return relative.to_owned();
    }

    let path_parts: Vec<&str> = path.split('/').collect();
    let root_parts: Vec<&str> = root_directory.split('/').collect();
    let shared = path_parts
        .iter()
        .zip(&root_parts)
        .take_while(|(from_path, from_root)| from_path == from_root)
        .count();

    if shared == 0 {
        return path_parts.last().unwrap_or(&path).to_string();
    }

    let mut relative = "../".repeat(root_parts.len().saturating_sub(shared));
    relative.push_str(&path_parts.get(shared..).unwrap_or_default().join("/"));
    relative
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_inside_the_project_root_loses_the_prefix() {
        assert_eq!(make_relative("D:/proj/src/utils.lua", "D:/proj"), "src/utils.lua");
        // A trailing separator on the root must not survive into the path.
        assert_eq!(make_relative("D:/proj/main.lua", "D:/proj/"), "main.lua");
    }

    /// A vendored dependency above the root keeps its shape, but nothing from
    /// the root's own side of the tree is emitted.
    #[test]
    fn path_above_the_project_root_walks_up() {
        assert_eq!(
            make_relative("D:/work/shared/util.lua", "D:/work/proj/src"),
            "../../shared/util.lua"
        );
        assert_eq!(make_relative("D:/work/sibling/a.lua", "D:/work/proj"), "../sibling/a.lua");
    }

    /// A path on another drive or share can't be made relative, and an
    /// absolute path must never reach the output.
    #[test]
    fn path_without_a_shared_root_keeps_only_the_file_name() {
        assert_eq!(make_relative("E:/elsewhere/lib.lua", "D:/proj"), "lib.lua");
    }

    /// The root of a UNC path is `//server/share`, so another server or share
    /// shares no root, and the leading empty segments don't count as one.
    #[test]
    fn bug_path_on_another_unc_share_keeps_only_the_file_name() {
        assert_eq!(make_relative("//server-a/share/lib.lua", "//server-b/proj"), "lib.lua");
        assert_eq!(make_relative("//server/other/lib.lua", "//server/share/proj"), "lib.lua");
    }

    /// `luck bundle main.lua` passes the entry's parent, which is empty for a
    /// bare file name.
    #[test]
    fn empty_project_root_resolves_to_the_working_directory() {
        let working_directory = normalize_path(&env::current_dir().expect("cwd"));
        assert_eq!(absolute_project_root(Path::new("")), working_directory);
        assert_eq!(absolute_project_root(Path::new(".")), working_directory);

        let root = absolute_project_root(Path::new(""));
        assert_eq!(make_relative(&format!("{root}/main.lua"), &root), "main.lua");
    }
}
