//! Committed size tracking, modeled on oxc's `tasks/minsize`.
//!
//! Minified and gzipped sizes for every benchmark file and project go to
//! `minsize.snap` next to this crate's `Cargo.toml`, and CI checks them. Byte
//! counts are exact, so any size regression or win shows up as a diff. Each
//! file is minified twice, and a second pass that changes it fails the run.
//!
//! Both tests are `#[ignore]`d because they fetch the benchmark files (over the
//! network on first run) and minify all of them. The benchmark workflow runs
//! the check.

use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;

use flate2::Compression;
use flate2::write::GzEncoder;
use luck_allocator::Allocator;
use luck_benchmark_files::{test_files, test_projects};
use luck_minifier::TransformConfig;
use luck_token::LuaVersion;

const SNAPSHOT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/minsize.snap");
const REGENERATE_COMMAND: &str =
    "cargo test -p luck_benchmark --test it regenerate_minsize -- --ignored";

struct Row {
    name: String,
    original_bytes: usize,
    minified_bytes: usize,
    gzip_bytes: usize,
}

fn measure(
    allocator: &mut Allocator,
    name: &str,
    source: &[u8],
    version: LuaVersion,
) -> Result<Row, Box<dyn Error>> {
    // Downloaded benchmark files keep whatever line endings upstream committed.
    let normalized: Vec<u8> = source
        .iter()
        .enumerate()
        .filter(|&(i, &b)| b != b'\r' || source.get(i + 1) != Some(&b'\n'))
        .map(|(_, &b)| b)
        .collect();
    let config = TransformConfig::default();
    allocator.reset();
    let minified = luck_minifier::minify(allocator, &normalized, version, &config)
        .map_err(|errors| format!("{name} failed to minify: {errors:?}"))?;
    let again = luck_minifier::minify(allocator, &minified, version, &config)
        .map_err(|errors| format!("{name}'s minified output failed to minify: {errors:?}"))?;
    if again != minified {
        return Err(format!("{name}'s minified output changed when minified again").into());
    }
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&minified)?;
    Ok(Row {
        name: name.to_owned(),
        original_bytes: normalized.len(),
        minified_bytes: minified.len(),
        gzip_bytes: encoder.finish()?.len(),
    })
}

/// One row per file and one per project, which sums its files.
fn collect_rows() -> Result<Vec<Row>, Box<dyn Error>> {
    let mut allocator = Allocator::new();
    let mut rows = Vec::new();
    for file in test_files() {
        let version = file.target.version();
        rows.push(measure(&mut allocator, file.file_name, &file.source_text, version)?);
    }
    for project in test_projects() {
        let version = project.target.version();
        let file_rows = project
            .files
            .iter()
            .map(|file| measure(&mut allocator, &file.relative_path, &file.source_text, version))
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

/// The regenerator and the up-to-date check both call this, so they can't
/// disagree on the format.
fn minsize_snapshot() -> Result<String, Box<dyn Error>> {
    let mut snapshot = String::new();
    writeln!(
        snapshot,
        "\
Minified-size tracking over the benchmark files. Sizes are exact bytes;
gzip is the minified output at the default flate2 level. Regenerate:
{REGENERATE_COMMAND}
"
    )?;
    writeln!(
        snapshot,
        "{:>10} | {:>10} | {:>10} | {:>7} | File",
        "Original", "Minified", "Gzip", "Ratio"
    )?;
    for row in collect_rows()? {
        let minified = f64::from(u32::try_from(row.minified_bytes)?);
        let original = f64::from(u32::try_from(row.original_bytes)?);
        let ratio = minified / original * 100.0;
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
#[ignore = "fetches the benchmark files and minifies all of them; run explicitly or in CI"]
fn minsize_is_up_to_date() {
    let committed = fs::read_to_string(SNAPSHOT_PATH).expect("read minsize.snap");
    assert_eq!(
        committed,
        minsize_snapshot().expect("build minsize snapshot"),
        "minsize.snap is stale; regenerate it with `{REGENERATE_COMMAND}`"
    );
}
