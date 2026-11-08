//! Shared source-file loading with SIMD UTF-8 validation.

use std::path::Path;
use std::{fs, io};

use simdutf8::basic::from_utf8;

/// Reads a source file to a `String`, validating UTF-8 with `simdutf8`.
///
/// Fails like `fs::read_to_string`: invalid UTF-8 is an `InvalidData` error
/// with the same message.
///
/// # Errors
///
/// Returns the read failure, or `InvalidData` when the file isn't UTF-8.
#[expect(unsafe_code, reason = "the safe conversion would validate every source file twice")]
pub fn read_source_file<P>(path: P) -> io::Result<String>
where
    P: AsRef<Path>,
{
    let bytes = fs::read(path)?;
    if from_utf8(&bytes).is_err() {
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
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ok.lua");
        fs::write(&path, "local x = 1 -- \u{00e9}\n").unwrap();
        assert_eq!(read_source_file(&path).unwrap(), "local x = 1 -- \u{00e9}\n");
    }

    #[test]
    fn invalid_utf8_matches_read_to_string_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bad.lua");
        fs::write(&path, [0x6c, 0xff, 0xfe]).unwrap();
        let simd_error = read_source_file(&path).unwrap_err();
        let std_error = fs::read_to_string(&path).unwrap_err();
        assert_eq!(simd_error.kind(), std_error.kind());
        assert_eq!(simd_error.to_string(), std_error.to_string());
    }

    #[test]
    fn missing_file_is_not_found() {
        let directory = tempfile::tempdir().unwrap();
        let missing = read_source_file(directory.path().join("does_not_exist.lua")).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    }
}
