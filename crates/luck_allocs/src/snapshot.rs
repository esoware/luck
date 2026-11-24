use std::fmt::{self, Write as _};
use std::path::Path;
use std::{fs, io};

use humansize::{DECIMAL, format_size};
use luck_benchmark_files::TestFile;
use saphyr::{LoadableYamlNode as _, SafelyIndex as _, Yaml};

use crate::stats::{Metric, MetricKind, StageStats};

/// Writes one stage's snapshot as a YAML mapping per file.
///
/// It's YAML so the committed values can be read back for
/// [`Metric::snapshot_value`], and written by hand to keep the layout's git
/// diffs small.
pub(crate) fn write_snapshot(path: &Path, entries: &[(&TestFile, StageStats)]) -> io::Result<()> {
    let committed = fs::read_to_string(path).unwrap_or_default();
    let committed_documents = Yaml::load_from_str(&committed).unwrap_or_default();
    let committed_document = committed_documents.first();
    let mut snapshot = String::new();
    for (i, (file, stats)) in entries.iter().enumerate() {
        if i > 0 {
            snapshot.push('\n');
        }
        let committed_file = committed_document.get(file.file_name);
        let metrics = stats.metrics(file.source_text.len());
        render_entry(&mut snapshot, file.file_name, &metrics, committed_file)
            .map_err(io::Error::other)?;
    }
    fs::write(path, snapshot)
}

/// Writes one entry's metrics under its name.
///
/// One metric per line and no alignment, so a changed value is a one-line
/// diff and a new metric reformats nothing. The name sits at column 0, so it
/// heads git's hunks. A byte count is the exact number, which is what diffs,
/// with its readable size in a comment.
pub(crate) fn render_entry(
    snapshot: &mut String,
    name: &str,
    metrics: &[Metric],
    committed: Option<&Yaml<'_>>,
) -> fmt::Result {
    writeln!(snapshot, "{name}:")?;
    for metric in metrics {
        let committed_value = committed.get(metric.label).and_then(Yaml::as_integer);
        let value = metric.snapshot_value(committed_value);
        match metric.kind {
            MetricKind::Count => writeln!(snapshot, "  {}: {value}", metric.label)?,
            MetricKind::Bytes { .. } => {
                writeln!(
                    snapshot,
                    "  {}: {value} # {}",
                    metric.label,
                    format_size(value, DECIMAL)
                )?;
            }
        }
    }
    Ok(())
}
