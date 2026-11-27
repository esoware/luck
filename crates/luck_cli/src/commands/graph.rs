use std::borrow::Cow;
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use luck_allocator::ThreadAllocators;
use luck_bundler::{DependencyGraph, DynamicRequire, ModuleId};
use luck_resolver::to_slash_lossy;
use serde_json::{Map, Value, json};

use crate::exit::{Exit, fail_with_diags};
use crate::project::{bundle_options, bundle_resolver, existing_file, one_shot_project};
use crate::reporter::print_files;
use crate::verbosity::Verbosity;

#[derive(Clone, ValueEnum)]
enum GraphFormat {
    Json,
    Dot,
}

#[derive(Args)]
pub(crate) struct GraphArgs {
    /// Entry file.
    entry: String,

    /// Project config [default: discovered from entry directory].
    #[arg(short = 'c', long = "config", value_name = "PATH")]
    config: Option<PathBuf>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = GraphFormat::Json)]
    format: GraphFormat,

    /// How to treat require(expr): error, warn, or allow [default: from luck.json].
    #[arg(long = "dynamic-require", value_name = "MODE")]
    dynamic_require: Option<DynamicRequire>,
}

impl GraphArgs {
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        let entry_path = existing_file(&self.entry)?;
        let project = one_shot_project(self.config.as_deref(), entry_path)?;
        let options = bundle_options(&project, self.dynamic_require);

        let arenas = ThreadAllocators::new();
        let dependency_graph =
            DependencyGraph::build(&arenas, &bundle_resolver(), entry_path, &options)
                .map_err(|errors| fail_with_diags(&errors))?;

        if verbosity != Verbosity::Quiet {
            print_files(&dependency_graph.warnings);
        }

        match self.format {
            GraphFormat::Json => print_graph_json(&dependency_graph),
            GraphFormat::Dot => print_graph_dot(&dependency_graph),
        }
        Ok(())
    }
}

fn print_graph_json(dependency_graph: &DependencyGraph<'_>) {
    let module_path = |id: ModuleId| to_slash_lossy(&dependency_graph[id].path);

    let mut modules_map = Map::new();
    for module in &dependency_graph.modules {
        let requires: Vec<&str> =
            module.dependencies.iter().map(|dependency| dependency.require_string).collect();
        let resolved_paths: Vec<Cow<'_, str>> =
            module.dependencies.iter().map(|dependency| module_path(dependency.target)).collect();

        modules_map.insert(
            to_slash_lossy(&module.path).into_owned(),
            json!({
                "requires": requires,
                "resolved_deps": resolved_paths,
            }),
        );
    }

    let ordered_paths: Vec<Cow<'_, str>> =
        dependency_graph.topological_order.iter().map(|id| module_path(*id)).collect();

    let output = json!({
        "entry": module_path(dependency_graph.entry_id),
        "modules": Value::Object(modules_map),
        "order": ordered_paths,
    });

    println!("{output:#}");
}

fn print_graph_dot(dependency_graph: &DependencyGraph<'_>) {
    // A double quote is the only character a DOT string escapes.
    let quoted = |path: &Path| to_slash_lossy(path).replace('"', "\\\"");
    println!("digraph dependencies {{");
    for module in &dependency_graph.modules {
        for dependency in &module.dependencies {
            let target = &dependency_graph[dependency.target];
            println!("  \"{}\" -> \"{}\";", quoted(&module.path), quoted(&target.path));
        }
    }
    println!("}}");
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::{GraphArgs, GraphFormat};
    use crate::cli::Cli;
    use crate::commands::Command;

    #[test]
    fn graph_accepts_an_entry_and_a_format() {
        let cli = Cli::try_parse_from(["luck", "graph", "src/main.luau", "--format", "dot"])
            .expect("graph parses");
        let Command::Graph(GraphArgs { entry, format, .. }) = cli.command else {
            panic!("expected Command::Graph");
        };
        assert_eq!(entry, "src/main.luau");
        assert!(matches!(format, GraphFormat::Dot));
    }
}
