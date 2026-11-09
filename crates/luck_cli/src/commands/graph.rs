//! `luck graph` - print the dependency graph as JSON or Graphviz DOT.

use clap::{Args, ValueEnum};
use luck_bundler::{BundleOptions, DependencyGraph, ModuleId};
use luck_core::{DynamicRequire, LuaTarget};

use crate::exit::{Exit, fail_with_diags};
use crate::project::{
    config_for_one_shot, entry_and_root, resolve_configured_target, resolve_dynamic_require,
};
use crate::render::print_diags;
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

    /// Lua target [default: from the entry's extension and luck.json].
    #[arg(short = 't', long = "target", value_name = "TARGET")]
    target: Option<LuaTarget>,

    /// Search path template (repeatable).
    #[arg(short = 's', long = "search-path", value_name = "PATTERN")]
    search_paths: Vec<String>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = GraphFormat::Json)]
    format: GraphFormat,

    /// How to treat require(expr): error, warn, or allow.
    #[arg(long = "dynamic-require", value_name = "MODE")]
    dynamic_require: Option<DynamicRequire>,
}

impl GraphArgs {
    /// # Errors
    ///
    /// Returns [`Exit::Usage`] on a bad config or entry path, and
    /// [`Exit::Failure`] when the graph can't be built.
    pub(crate) fn run(self, verbosity: Verbosity) -> Result<(), Exit> {
        // `graph` takes no `-c`, so its config is always the discovered one.
        let config = config_for_one_shot(None, &self.entry)?;
        let target = resolve_configured_target(&config, self.target, &self.entry)?;
        let dynamic_require = resolve_dynamic_require(&config, self.dynamic_require);

        let (entry_path, project_root) = entry_and_root(&self.entry)?;
        let version = target.lua_version();

        let dependency_graph = DependencyGraph::build(
            entry_path,
            &BundleOptions {
                version,
                search_paths: &self.search_paths,
                project_root,
                dynamic_require,
            },
        )
        .map_err(|errors| fail_with_diags(&errors, version))?;

        if verbosity != Verbosity::Quiet {
            print_diags(&dependency_graph.warnings, version);
        }

        match self.format {
            GraphFormat::Json => print_graph_json(&dependency_graph),
            GraphFormat::Dot => print_graph_dot(&dependency_graph),
        }
        Ok(())
    }
}

fn print_graph_json(dependency_graph: &DependencyGraph) {
    use serde_json::{Map, Value, json};

    let module_path =
        |id: ModuleId| dependency_graph.modules.get(id.0).map(|module| module.path.as_str());

    let mut modules_map = Map::new();
    for module in &dependency_graph.modules {
        let requires: Vec<&str> = module
            .dependencies
            .iter()
            .map(|dependency| dependency.require_string.as_str())
            .collect();
        let resolved_paths: Vec<Option<&str>> =
            module.dependencies.iter().map(|dependency| module_path(dependency.target)).collect();

        modules_map.insert(
            module.path.clone(),
            json!({
                "requires": requires,
                "resolved_deps": resolved_paths,
            }),
        );
    }

    let ordered_paths: Vec<Option<&str>> =
        dependency_graph.topological_order.iter().map(|id| module_path(*id)).collect();

    let output = json!({
        "entry": module_path(dependency_graph.entry_id),
        "modules": Value::Object(modules_map),
        "order": ordered_paths,
    });

    println!("{output:#}");
}

fn print_graph_dot(dependency_graph: &DependencyGraph) {
    println!("digraph dependencies {{");
    for module in &dependency_graph.modules {
        for dependency in &module.dependencies {
            if let Some(target) = dependency_graph.modules.get(dependency.target.0) {
                println!("  \"{}\" -> \"{}\";", module.path, target.path);
            }
        }
    }
    println!("}}");
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use luck_core::LuaTarget;

    use super::{GraphArgs, GraphFormat};
    use crate::cli::{Cli, Command};

    #[test]
    fn graph_accepts_target_and_format() {
        let cli =
            Cli::try_parse_from(["luck", "graph", "src/main.lua", "-t", "54", "--format", "dot"])
                .expect("graph parses");
        let Command::Graph(GraphArgs { entry, target, format, .. }) = cli.command else {
            panic!("expected Command::Graph");
        };
        assert_eq!(entry, "src/main.lua");
        assert_eq!(target, Some(LuaTarget::Lua54));
        assert!(matches!(format, GraphFormat::Dot));
    }
}
