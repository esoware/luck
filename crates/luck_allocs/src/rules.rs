use std::path::Path;
use std::{fs, io};

use luck_allocator::Allocator;
use luck_benchmark_files::TestFile;
use luck_linter::{CategoryLevels, FixKind, LintConfig, Linter, RuleId, RuleLevel};
use luck_stdlib::library_for;

use crate::every_rule;
use crate::snapshot::render_entry;
use crate::stats::{StageStats, record_stats_in};

/// Writes `allocs_linter_rules.yaml`: each rule's allocations on each file,
/// heaviest first by heap bytes, then heap allocations, then name.
///
/// Each rule runs alone over one analysis per file, which parsing, analysis,
/// and compiling its linter happen outside of, so its numbers are its own.
/// Its `arena size` is how far the arena grew while it ran.
pub(crate) fn write_report(allocator: &mut Allocator, files: &[TestFile]) -> io::Result<()> {
    let every_rule = every_rule();
    let linters: Vec<(RuleId, Linter)> =
        RuleId::ALL.into_iter().map(|rule| (rule, only(rule))).collect();
    let mut report = String::new();
    for file in files {
        allocator.reset();
        let parse = luck_parser::parse(allocator, &file.source_text);
        let analysis = every_rule.analyze(&parse, library_for(file.has_roblox));
        let mut entries: Vec<(&str, StageStats)> = linters
            .iter()
            .map(|(rule, linter)| {
                let arena_bytes_before = allocator.used_bytes();
                let ((), mut stats) = record_stats_in(allocator, || {
                    drop(linter.run_analyzed(&analysis, FixKind::empty(), None, &|| false));
                });
                stats.arena_used_bytes -= arena_bytes_before;
                (rule.name(), stats)
            })
            .collect();
        entries.sort_unstable_by(|(left_name, left), (right_name, right)| {
            let (left_heap, right_heap) = (&left.heap, &right.heap);
            right_heap
                .alloc_bytes
                .cmp(&left_heap.alloc_bytes)
                .then_with(|| right_heap.allocs.cmp(&left_heap.allocs))
                .then_with(|| left_name.cmp(right_name))
        });

        report.push_str(file.file_name);
        report.push_str(":\n");
        for (name, stats) in entries {
            render_entry(&mut report, "  ", name, stats.metrics(), None)
                .map_err(io::Error::other)?;
        }
        report.push('\n');
    }
    fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("allocs_linter_rules.yaml"), report)
}

fn only(rule: RuleId) -> Linter {
    let mut config =
        LintConfig { categories: CategoryLevels::all(RuleLevel::Off), ..LintConfig::default() };
    config.rules.set_level(rule, RuleLevel::Warn);
    Linter::new(config)
}
