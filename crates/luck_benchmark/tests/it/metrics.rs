//! Committed size tracking, modeled on oxc's `tasks/minsize`.
//!
//! Minified and gzipped sizes for every corpus input go to `minsize.snap`
//! next to this crate's `Cargo.toml`, and CI checks them. Byte counts are
//! exact, so any size regression or win shows up as a diff.
//!
//! Both tests are `#[ignore]`d because they fetch the corpus (over the
//! network on first run) and minify all of it. The benchmark workflow runs
//! the check.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test panics to fail, so a Panics section says nothing"
)]

use core::error::Error;
use core::fmt::Write as _;
use std::fs;
use std::io::Write as _;

use flate2::Compression;
use flate2::write::GzEncoder;
use luck_benchmark::{test_files, test_projects};
use luck_core::{LuaTarget, TransformConfig};

const SNAPSHOT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/minsize.snap");
const REGENERATE_COMMAND: &str =
    "cargo test -p luck_benchmark --test it regenerate_minsize -- --ignored";

/// Byte counts for a file or a whole project.
struct Row {
    name: String,
    original_bytes: usize,
    minified_bytes: usize,
    gzip_bytes: usize,
}

/// # Errors
///
/// Fails when the encoder can't write.
fn gzipped_byte_count(text: &str) -> Result<usize, Box<dyn Error>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(text.as_bytes())?;
    Ok(encoder.finish()?.len())
}

/// # Errors
///
/// Fails when `source` doesn't minify or the result can't be compressed.
fn measure(name: &str, source: &str, target: LuaTarget) -> Result<Row, Box<dyn Error>> {
    // Windows checkouts may carry CRLF in include_str! fixtures; normalize
    // so byte counts match CI.
    let normalized = source.replace("\r\n", "\n");
    let config = TransformConfig::default();
    let minified = luck_minifier::minify(&normalized, target.lua_version(), &config)
        .map_err(|errors| format!("{name} failed to minify: {errors:?}"))?;
    Ok(Row {
        name: name.to_owned(),
        original_bytes: normalized.len(),
        minified_bytes: minified.len(),
        gzip_bytes: gzipped_byte_count(&minified)?,
    })
}

/// One row per file and one per project, which sums its files.
///
/// # Errors
///
/// Fails when any corpus input can't be measured.
fn collect_rows() -> Result<Vec<Row>, Box<dyn Error>> {
    let mut rows = Vec::new();
    for file in test_files() {
        rows.push(measure(file.file_name, &file.source_text, file.target)?);
    }
    for project in test_projects() {
        let file_rows = project
            .files
            .iter()
            .map(|file| measure(&file.relative_path, &file.source_text, project.target))
            .collect::<Result<Vec<_>, _>>()?;
        rows.push(Row {
            name: format!("{} ({} files)", project.name, project.files.len()),
            original_bytes: file_rows.iter().map(|row| row.original_bytes).sum(),
            minified_bytes: file_rows.iter().map(|row| row.minified_bytes).sum(),
            gzip_bytes: file_rows.iter().map(|row| row.gzip_bytes).sum(),
        });
    }
    Ok(rows)
}

/// Renders the snapshot.
///
/// The regenerator and the up-to-date check both call this, so they can't
/// disagree on the format.
///
/// # Errors
///
/// Fails when a corpus input can't be measured.
fn minsize_snapshot() -> Result<String, Box<dyn Error>> {
    let mut snapshot = String::new();
    writeln!(
        snapshot,
        "Minified-size tracking over the bench corpus. Sizes are exact bytes;\n\
         gzip is the minified output at the default flate2 level. Regenerate:\n\
         {REGENERATE_COMMAND}\n"
    )?;
    writeln!(
        snapshot,
        "{:>10} | {:>10} | {:>10} | {:>7} | File",
        "Original", "Minified", "Gzip", "Ratio"
    )?;
    for row in collect_rows()? {
        let minified = f64::from(u32::try_from(row.minified_bytes)?);
        let original = f64::from(u32::try_from(row.original_bytes)?);
        #[expect(clippy::float_arithmetic, reason = "minsize.snap pins the rounding of this ratio")]
        let ratio = minified / original * 100.0f64;
        writeln!(
            snapshot,
            "{:>10} | {:>10} | {:>10} | {ratio:>6.2}% | {}",
            row.original_bytes, row.minified_bytes, row.gzip_bytes, row.name
        )?;
    }
    Ok(snapshot)
}

#[test]
#[ignore = "writes the committed snapshot; run with --ignored to refresh"]
fn regenerate_minsize() {
    let snapshot = minsize_snapshot().expect("build minsize snapshot");
    fs::write(SNAPSHOT_PATH, snapshot).expect("write minsize.snap");
}

#[test]
#[ignore = "fetches the corpus and minifies all of it; run explicitly or in CI"]
fn minsize_is_up_to_date() {
    let committed = fs::read_to_string(SNAPSHOT_PATH).expect("read minsize.snap");
    assert_eq!(
        committed.replace("\r\n", "\n"),
        minsize_snapshot().expect("build minsize snapshot"),
        "minsize.snap is stale; regenerate it with `{REGENERATE_COMMAND}`"
    );
}
