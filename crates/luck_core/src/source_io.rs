//! Shared source-file loading with SIMD UTF-8 validation.

use std::io;
use std::path::Path;

/// Reads a source file to `String`, validating UTF-8 with `simdutf8`.
/// Matches `fs::read_to_string` behavior: invalid UTF-8 is an
/// `InvalidData` error with the same message.
pub fn read_source_file(path: impl AsRef<Path>) -> io::Result<String> {
    let bytes = std::fs::read(path)?;
    if simdutf8::basic::from_utf8(&bytes).is_err() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "stream did not contain valid UTF-8",
        ));
    }
    // SAFETY: `bytes` was validated as UTF-8 above.
    Ok(unsafe { String::from_utf8_unchecked(bytes) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_utf8_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.lua");
        std::fs::write(&path, "local x = 1 -- \u{00e9}\n").unwrap();
        assert_eq!(
            read_source_file(&path).unwrap(),
            "local x = 1 -- \u{00e9}\n"
        );
    }

    #[test]
    fn invalid_utf8_matches_read_to_string_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.lua");
        std::fs::write(&path, [0x6c, 0xff, 0xfe]).unwrap();
        let simd_error = read_source_file(&path).unwrap_err();
        let std_error = std::fs::read_to_string(&path).unwrap_err();
        assert_eq!(simd_error.kind(), std_error.kind());
        assert_eq!(simd_error.to_string(), std_error.to_string());
    }

    #[test]
    fn missing_file_is_not_found() {
        let missing = read_source_file("does_not_exist.lua").unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    }
}
