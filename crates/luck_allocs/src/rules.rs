use std::path::Path;
use std::{fs, io};

use luck_allocator::Allocator;
use luck_benchmark_files::TestFile;
use luck_linter::{CategoryLevels, FixKind, LintConfig, RuleLevel};

use crate::every_rule;
use crate::snapshot::render_entry;
use crate::stats::{StageStats, record_stats_in};

/// Writes `allocs_linter_rules.yaml`: each rule's allocations on each file,
/// heaviest first by heap bytes, then heap allocations, then name.
///
/// Each rule runs alone over one analysis per file, which parsing, analysis,
/// and configuration happen outside of, so its numbers are its own. Its
/// `arena size` is how far the arena grew while it ran.
///
/// # Errors
///
/// Fails when the report can't be written.
pub(crate) fn run(allocator: &mut Allocator, files: &[TestFile]) -> io::Result<()> {
    let every_rule = every_rule();
    let mut report = String::new();
    for file in files {
        allocator.reset();
        let version = file.target.version();
        let parse = luck_parser::parse(allocator, &file.source_text, version);
        let analysis = luck_linter::analyze(&parse, file.target, &every_rule);
        let mut entries: Vec<(&str, StageStats)> = luck_linter::rules()
            .map(|rule| {
                let config = only(rule.name());
                let arena_bytes_before = allocator.used_bytes();
                let ((), mut stats) = record_stats_in(allocator, || {
                    drop(luck_linter::lint_analyzed(
                        &parse,
                        &analysis,
                        &config,
                        FixKind::empty(),
                        &|| false,
                    ));
                });
                stats.arena_used_bytes = allocator.used_bytes().saturating_sub(arena_bytes_before);
                (rule.name(), stats)
            })
            .collect();
        entries.sort_unstable_by(|(left_name, left), (right_name, right)| {
            let (left_heap, right_heap) = (&left.counters.heap, &right.counters.heap);
            right_heap
                .alloc_bytes
                .cmp(&left_heap.alloc_bytes)
                .then_with(|| right_heap.allocs.cmp(&left_heap.allocs))
                .then_with(|| left_name.cmp(right_name))
        });

        report.push_str(file.file_name);
        report.push_str(":\n");
        for (name, stats) in entries {
            let mut metrics = stats.metrics(file.source_text.len());
            metrics.retain(|metric| metric.label != "file size");
            let mut entry = String::new();
            render_entry(&mut entry, name, &metrics, None).map_err(io::Error::other)?;
            for line in entry.lines() {
                report.push_str("  ");
                report.push_str(line);
                report.push('\n');
            }
        }
        report.push('\n');
    }
    fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("allocs_linter_rules.yaml"), report)
}

fn only(rule: &str) -> LintConfig {
    let mut config = LintConfig {
        categories: CategoryLevels {
            correctness: Some(RuleLevel::Off),
            suspicious: Some(RuleLevel::Off),
            style: Some(RuleLevel::Off),
            performance: Some(RuleLevel::Off),
        },
        ..LintConfig::default()
    };
    config.rules.set_level(rule, RuleLevel::Warn);
    config
}
