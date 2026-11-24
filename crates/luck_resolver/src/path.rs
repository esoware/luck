use std::borrow::Cow;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

/// `path` with forward slashes on every platform, the form luck writes a path
/// out in, with any name that isn't Unicode lossily converted.
pub fn to_slash_lossy(path: &Path) -> Cow<'_, str> {
    let lossy = path.to_string_lossy();
    #[cfg(windows)]
    if lossy.contains('\\') {
        return Cow::Owned(lossy.replace('\\', "/"));
    }
    lossy
}

/// `path` without the `\\?\` that Windows' `canonicalize` puts before it,
/// when the path it marks is one a shorter form names too.
#[cfg(windows)]
pub(crate) fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    use std::ffi::OsString;
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return path;
    };
    let mut plain = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:", char::from(drive))),
        Prefix::VerbatimUNC(server, share) => {
            let mut unc = OsString::from(r"\\");
            unc.push(server);
            unc.push(r"\");
            unc.push(share);
            PathBuf::from(unc)
        }
        // A volume GUID path is kept, since nothing shorter is absolute.
        Prefix::Verbatim(_) | Prefix::DeviceNS(_) | Prefix::UNC(..) | Prefix::Disk(_) => {
            return path;
        }
    };
    plain.push(components.as_path());
    plain
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn strip_verbatim_prefix_gives_the_plain_form() {
        for (verbatim, plain) in [
            (r"\\?\C:\Users\user\main.lua", r"C:\Users\user\main.lua"),
            (r"\\?\C:\", r"C:\"),
            (r"\\?\UNC\server\share\main.lua", r"\\server\share\main.lua"),
            (r"C:\Users\user\main.lua", r"C:\Users\user\main.lua"),
            (r"\\server\share\main.lua", r"\\server\share\main.lua"),
        ] {
            assert_eq!(strip_verbatim_prefix(PathBuf::from(verbatim)).as_os_str(), plain);
        }
    }

    #[test]
    fn strip_verbatim_prefix_keeps_a_volume_guid_path() {
        let volume_path = r"\\?\Volume{12345678-1234-1234-1234-123456789abc}\src\main.lua";
        let stripped = strip_verbatim_prefix(PathBuf::from(volume_path));
        assert_eq!(stripped.as_os_str(), volume_path);
        assert!(stripped.is_absolute());
    }
}
