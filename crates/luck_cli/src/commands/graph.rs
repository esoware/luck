//! `luck graph` - print the dependency graph as JSON or Graphviz DOT.

use crate::output::{build_file_cache, current_dir_or_exit, fail_with_diagnostics};
use crate::project::resolve_explicit_target;
use crate::render::render_diagnostics;
use crate::{EXIT_SUCCESS, EXIT_USAGE, Verbosity};
use clap::{Args, ValueEnum};
use luck_bundler::graph::{DependencyGraph, build_graph};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Clone, ValueEnum)]
pub(crate) enum GraphFormat {
    Json,
    Dot,
}

#[derive(Args)]
pub(crate) struct GraphArgs {
    /// Entry file
    entry: String,

    /// Lua target [default: inferred from entry extension]
    #[arg(short = 't', long = "target", value_name = "TARGET")]
    target: Option<String>,

    /// Search path template (repeatable)
    #[arg(short = 's', long = "search-path", value_name = "PATTERN")]
    search_path: Vec<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = GraphFormat::Json)]
    format: GraphFormat,
}

impl GraphArgs {
    pub(crate) fn run(self, verbosity: Verbosity) -> ExitCode {
        let target = resolve_explicit_target(self.target.as_deref(), &self.entry);

        let entry_path = PathBuf::from(&self.entry);
        if !entry_path.is_file() {
            eprintln!("Error: entry file not found: {}", self.entry);
            return ExitCode::from(EXIT_USAGE);
        }

        let cwd = current_dir_or_exit();

        match build_graph(&entry_path, target, &self.search_path, &cwd) {
            Ok(dep_graph) => {
                if !dep_graph.warnings.is_empty() && verbosity != Verbosity::Quiet {
                    let mut cache = build_file_cache(&dep_graph.warnings);
                    render_diagnostics(&dep_graph.warnings, &mut cache);
                }

                match self.format {
                    GraphFormat::Json => print_graph_json(&dep_graph),
                    GraphFormat::Dot => print_graph_dot(&dep_graph),
                }
                ExitCode::from(EXIT_SUCCESS)
            }
            Err(errors) => fail_with_diagnostics(&errors, None),
        }
    }
}

fn print_graph_json(dep_graph: &DependencyGraph) {
    use serde_json::{Map, Value, json};

    let entry_path = &dep_graph.modules[dep_graph.entry_id.0].path;

    let mut modules_map = Map::new();
    for module in &dep_graph.modules {
        let requires: Vec<&str> = module
            .dependencies
            .iter()
            .map(|dep| dep.require_string.as_str())
            .collect();
        let resolved_deps: Vec<&str> = module
            .dependencies
            .iter()
            .map(|dep| dep.resolved_path.as_str())
            .collect();

        modules_map.insert(
            module.path.clone(),
            json!({
                "requires": requires,
                "resolved_deps": resolved_deps,
            }),
        );
    }

    let order: Vec<&str> = dep_graph
        .topo_order
        .iter()
        .map(|id| dep_graph.modules[id.0].path.as_str())
        .collect();

    let output = json!({
        "entry": entry_path,
        "modules": Value::Object(modules_map),
        "order": order,
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&output).expect("failed to serialize dependency graph")
    );
}

fn print_graph_dot(dep_graph: &DependencyGraph) {
    println!("digraph dependencies {{");
    for module in &dep_graph.modules {
        for dep in &module.dependencies {
            println!("  \"{}\" -> \"{}\";", module.path, dep.resolved_path);
        }
    }
    println!("}}");
}

#[cfg(test)]
mod tests {
    use super::{GraphArgs, GraphFormat};
    use crate::args::{Cli, Command};
    use clap::Parser;

    #[test]
    fn graph_accepts_target_and_format() {
        let cli = Cli::try_parse_from([
            "luck",
            "graph",
            "src/main.lua",
            "-t",
            "54",
            "--format",
            "dot",
        ])
        .expect("graph parses");
        match cli.command {
            Command::Graph(GraphArgs {
                entry,
                target,
                format,
                ..
            }) => {
                assert_eq!(entry, "src/main.lua");
                assert_eq!(target, Some("54".to_string()));
                assert!(matches!(format, GraphFormat::Dot));
            }
            _ => panic!("expected Command::Graph"),
        }
    }
}
