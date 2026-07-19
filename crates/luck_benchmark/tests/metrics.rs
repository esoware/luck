//! Committed size tracking, modeled on oxc's `tasks/minsize`: minified
//! and gzipped sizes for every corpus input, written to `minsize.snap`
//! next to this crate's `Cargo.toml` and checked in CI. Byte counts are
//! exact so any size regression or win shows up as a diff.
//!
//! Both tests are `#[ignore]`d because they fetch the corpus (network on
//! first run) and minify all of it; the benchmark workflow runs the check.

use std::io::Write as _;

use luck_benchmark::corpus::{test_files, test_projects};
use luck_core::TransformConfig;
use luck_core::types::LuaTarget;

const SNAPSHOT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/minsize.snap");

struct Row {
    name: String,
    original: usize,
    minified: usize,
    gzip: usize,
}

fn gzip_len(text: &str) -> usize {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(text.as_bytes()).expect("gzip write");
    encoder.finish().expect("gzip finish").len()
}

fn minify_len(name: &str, source: &str, target: LuaTarget) -> (usize, usize, usize) {
    // Windows checkouts may carry CRLF in include_str! fixtures; normalize
    // so byte counts match CI.
    let source = source.replace("\r\n", "\n");
    let config = TransformConfig::default();
    let minified = luck_minifier::minify(&source, target, &config, name)
        .unwrap_or_else(|errors| panic!("{name} failed to minify: {errors:?}"));
    (source.len(), minified.len(), gzip_len(&minified))
}

fn collect_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for file in test_files() {
        let (original, minified, gzip) = minify_len(file.file_name, &file.source_text, file.target);
        rows.push(Row {
            name: file.file_name.to_string(),
            original,
            minified,
            gzip,
        });
    }
    for project in test_projects() {
        let mut totals = Row {
            name: format!("{} ({} files)", project.name, project.files.len()),
            original: 0,
            minified: 0,
            gzip: 0,
        };
        for (name, source_text) in &project.files {
            let (original, minified, gzip) = minify_len(name, source_text, project.target);
            totals.original += original;
            totals.minified += minified;
            totals.gzip += gzip;
        }
        rows.push(totals);
    }
    rows
}

/// Single source of truth for the snapshot's serialized form; the
/// regenerator and the up-to-date checker both call this.
fn minsize_snapshot() -> String {
    let mut out = String::new();
    out.push_str(
        "Minified-size tracking over the bench corpus. Sizes are exact bytes;\n\
         gzip is the minified output at the default flate2 level. Regenerate:\n\
         cargo test -p luck_benchmark --test metrics regenerate_minsize -- --ignored\n\n",
    );
    out.push_str(&format!(
        "{:>10} | {:>10} | {:>10} | {:>7} | File\n",
        "Original", "Minified", "Gzip", "Ratio"
    ));
    for row in collect_rows() {
        let ratio = row.minified as f64 / row.original as f64 * 100.0;
        out.push_str(&format!(
            "{:>10} | {:>10} | {:>10} | {:>6.2}% | {}\n",
            row.original, row.minified, row.gzip, ratio, row.name
        ));
    }
    out
}

#[test]
#[ignore = "writes the committed snapshot; run with --ignored to refresh"]
fn regenerate_minsize() {
    std::fs::write(SNAPSHOT_PATH, minsize_snapshot()).expect("write minsize.snap");
}

#[test]
#[ignore = "fetches the corpus and minifies all of it; run explicitly or in CI"]
fn minsize_is_up_to_date() {
    let committed = std::fs::read_to_string(SNAPSHOT_PATH).expect("read minsize.snap");
    assert_eq!(
        committed.replace("\r\n", "\n"),
        minsize_snapshot(),
        "minsize.snap is stale; regenerate it with \
         `cargo test -p luck_benchmark --test metrics regenerate_minsize -- --ignored`"
    );
}
