use std::fmt::Write as _;
use std::ops::Range;
use std::path::PathBuf;
use std::{iter, mem};

use luck_allocator::Allocator;
use luck_diagnostics::FileDiagnostics;
use luck_token::{Span, chunk_header, ends_with_line_break, line_breaks};

use crate::graph::DependencyGraph;
use crate::loader::write_prelude;
use crate::module::ModuleInfo;
use crate::plan::{Binding, Item, Plan, Registration, WrappedModule, may_overflow};
use crate::rewrite::{LoaderNames, overflow_errors, rewrite_module};

/// Line 1 of the module's source lands on `bundle_start_line`, so a
/// runtime traceback line `L` inside the range maps back to source line
/// `L - bundle_start_line + 1`.
///
/// Require rewrites preserve the newline count of the call they replace, so
/// the correspondence is 1:1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMapEntry {
    /// 1-based first bundle line of the module body (inclusive).
    pub bundle_start_line: usize,
    /// 1-based last bundle line of the module body (inclusive).
    pub bundle_end_line: usize,
    /// The module's canonical path. The line map is local debug data, so it
    /// may name the host.
    pub path: PathBuf,
}

fn push_number(output: &mut String, number: usize) {
    let _ = write!(output, "{number}");
}

impl Registration {
    fn write(&self, prefix: &str, output: &mut String) {
        match *self {
            Self::Exporting { slot } => {
                output.extend([prefix, "modules["]);
                push_number(output, slot);
                output.extend(["]=", prefix, "exporting("]);
                push_number(output, slot);
                output.extend([",", prefix, "modules["]);
                push_number(output, slot);
                output.push_str("])\n");
            }
            Self::Entry { slot, is_exporting } => {
                output.extend([prefix, "modules["]);
                push_number(output, slot);
                output.push_str("]=");
                if is_exporting {
                    output.extend([prefix, "exporting("]);
                    push_number(output, slot);
                    output.extend([",", prefix, "entry)\n"]);
                } else {
                    output.extend([prefix, "entry\n"]);
                }
            }
        }
    }
}

/// One splice of a module's source.
pub(crate) enum Edit {
    /// A require becomes a call of the loader with the module's slot, padded
    /// with the line breaks the call spans.
    Require {
        slot: usize,
        line_breaks: usize,
    },
    Text(String),
}

/// The splices `module` takes in its source, by where they start.
fn module_edits(module: &ModuleInfo<'_>, plan: &Plan) -> Vec<(Span, Edit)> {
    let source = module.source;
    let mut edits: Vec<(Span, Edit)> = Vec::with_capacity(module.dependencies.len() + 1);
    edits.extend(module.dependencies.iter().filter_map(|dependency| {
        let call = dependency.call_span.text(source);
        if call.is_empty() {
            return None;
        }
        let slot = plan.slot(dependency);
        Some((dependency.call_span, Edit::Require { slot, line_breaks: line_breaks(call).count() }))
    }));
    module.exports.text_edits(&module.block, source, &plan.prefix, &mut edits);
    // Only a chunk may open with a shebang, and this body is inside a
    // function. The shebang's line break stays for the line map.
    edits.extend(module.shebang.map(|span| (span, Edit::Text(String::new()))));
    edits.sort_by_key(|(span, _)| span.start);
    edits
}

/// The bundle's text, which keeps every module's source as written, with
/// splices only where a require or an export is rewritten, and a line map
/// back to it.
///
/// A lone module with no requires is emitted verbatim.
///
/// A shebang is only valid on a chunk's first line, so the entry's moves to
/// the bundle's first line, which keeps the bundle runnable as a script, and
/// every other module's is dropped.
pub(crate) fn emit<'ast>(
    allocator: &'ast Allocator,
    graph: &mut DependencyGraph<'ast>,
) -> Result<(Vec<u8>, Vec<LineMapEntry>), Vec<FileDiagnostics>> {
    let entry = &graph[graph.entry_id];
    if graph.modules.len() == 1 && entry.dependencies.is_empty() {
        let line_map = vec![LineMapEntry {
            bundle_start_line: 1,
            // A final line break closes its line rather than opening an empty one.
            bundle_end_line: line_breaks(entry.source).count()
                + usize::from(!ends_with_line_break(entry.source)),
            path: entry.path.clone(),
        }];
        return Ok((entry.source.to_owned(), line_map));
    }

    let plan = Plan::new(graph);
    let names = LoaderNames::new(allocator, &plan.prefix);
    let mut edits: Vec<Vec<(Span, Edit)>> = Vec::with_capacity(graph.modules.len());
    let mut errors = Vec::new();
    for wrapped in plan.wrapped_modules() {
        let module = &mut graph[wrapped.id];
        // Taken before measuring the module rewrites its tree.
        edits.push(module_edits(module, &plan));
        if may_overflow(module) {
            let mut block = module.block.take();
            rewrite_module(allocator, &mut block, module, &plan, &names);
            errors.extend(overflow_errors(&block, module));
            module.block = block;
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    // The sources plus rough room for each module's wrapper and the loader.
    let capacity = graph.modules.iter().map(|module| module.source.len() + 96).sum::<usize>() + 512;
    let writer = write(graph, &plan, capacity, Bodies::Spliced(edits));
    Ok((writer.output, writer.line_map))
}

/// What a module's body is in the text [`skeleton`] writes: a call of this,
/// after the bundle's prefix, which no module's source holds.
pub(crate) const HOLE: &str = "body";

/// The bundle's text with each module's body written as a call of the
/// prefixed [`HOLE`], so the tree that holds the modules' own trees can take
/// everything else from the text's parse.
pub(crate) fn skeleton(graph: &DependencyGraph<'_>, plan: &Plan) -> Vec<u8> {
    let capacity = graph.modules.len() * 128 + 4096;
    write(graph, plan, capacity, Bodies::Holes).output
}

/// How [`write()`] writes each module's body.
enum Bodies {
    /// Each wrapped module's source with its splices, in bundle order.
    Spliced(Vec<Vec<(Span, Edit)>>),
    Holes,
}

fn write<'plan>(
    graph: &DependencyGraph<'_>,
    plan: &'plan Plan,
    capacity: usize,
    bodies: Bodies,
) -> BundleWriter<'plan> {
    let mut writer = BundleWriter {
        output: Vec::with_capacity(capacity),
        text: String::new(),
        next_line: 1,
        line_map: Vec::with_capacity(graph.modules.len()),
        plan,
    };
    let prefix = plan.prefix.as_str();

    let entry = &graph[graph.entry_id];
    writer.push(&chunk_header(entry.source, entry.shebang, entry.hot_comments.iter().copied()));

    writer.generate(|output| write_prelude(prefix, plan.has_export_runtime, output));

    let mut spliced = match bodies {
        Bodies::Spliced(edits) => Some(edits.into_iter()),
        Bodies::Holes => None,
    };
    for item in &plan.items {
        match item {
            Item::Module(wrapped) => {
                let module_edits = spliced
                    .as_mut()
                    .map(|edits| edits.next().expect("each wrapped module has its edits"));
                writer.push_wrapped(wrapped, &graph[wrapped.id], module_edits);
            }
            Item::Registration(registration) => {
                writer.generate(|output| registration.write(prefix, output));
            }
        }
    }

    writer.generate(|output| output.extend(["return ", prefix, "entry(...)\n"]));
    writer
}

struct BundleWriter<'plan> {
    /// Bytes, as the modules' sources are, which it copies as written.
    output: Vec<u8>,
    /// Where [`Self::generate`] writes the text it adds, which the output
    /// then takes, so every generated piece shares one buffer.
    text: String,
    /// 1-based line the next pushed character lands on.
    next_line: usize,
    line_map: Vec<LineMapEntry>,
    plan: &'plan Plan,
}

impl BundleWriter<'_> {
    fn push(&mut self, fragment: &[u8]) {
        self.output.extend_from_slice(fragment);
        self.next_line += line_breaks(fragment).count();
    }

    /// Writes generated text with `write`, counting its line breaks.
    fn generate(&mut self, write: impl FnOnce(&mut String)) {
        let mut text = mem::take(&mut self.text);
        text.clear();
        write(&mut text);
        self.push(text.as_bytes());
        self.text = text;
    }

    /// `edits` is `None` for a hole.
    fn push_wrapped(
        &mut self,
        wrapped: &WrappedModule,
        module: &ModuleInfo<'_>,
        edits: Option<Vec<(Span, Edit)>>,
    ) {
        let prefix = self.plan.prefix.as_str();
        self.generate(|output| {
            if let Binding::Slot(slot) = wrapped.binding {
                write_provenance(slot, &module.relative_path, output);
            }
            write_wrapper_opening(wrapped.binding, prefix, output);
            module.exports.write_table_declaration(prefix, output);
            output.push('\n');
        });
        if let Some(body_edits) = edits {
            self.push_body(module, &body_edits);
        } else {
            self.generate(|output| {
                output.extend([prefix, HOLE, "()\n"]);
                module.exports.write_closing(prefix, output);
            });
        }
        self.push(b"end\n");
    }

    /// A loader call keeps the newline count of the call it replaces, so the
    /// line map stays 1:1 for multiline requires too.
    fn push_body(&mut self, module: &ModuleInfo<'_>, edits: &[(Span, Edit)]) {
        let source = module.source;
        let first_line = self.next_line;
        let body_start = self.output.len();
        let mut cursor = 0;
        for (span, edit) in edits {
            let range = Range::<usize>::from(*span);
            self.push(&source[cursor..range.start]);
            self.push_edit(edit);
            cursor = range.end;
        }
        self.push(&source[cursor..]);

        if module.exports.has_values {
            if self.output.len() == body_start || !ends_with_line_break(&self.output) {
                self.push(b"\n");
            }
            let prefix = self.plan.prefix.as_str();
            self.generate(|output| module.exports.write_closing(prefix, output));
        }
        if self.output.len() > body_start && !ends_with_line_break(&self.output) {
            self.push(b"\n");
        }

        self.line_map.push(LineMapEntry {
            bundle_start_line: first_line,
            bundle_end_line: self.next_line - 1,
            path: module.path.clone(),
        });
    }

    fn push_edit(&mut self, edit: &Edit) {
        let prefix = self.plan.prefix.as_str();
        match edit {
            &Edit::Require { slot, line_breaks } => self.generate(|output| {
                output.extend([prefix, "require("]);
                push_number(output, slot);
                output.extend(iter::repeat_n('\n', line_breaks));
                output.push(')');
            }),
            Edit::Text(text) => self.push(text.as_bytes()),
        }
    }
}

/// The comment above the wrapper of the module in `slot`. A line break in a
/// file name would end the comment and turn the rest of the name into code,
/// so it's written as an escape.
fn write_provenance(slot: usize, relative_path: &str, output: &mut String) {
    output.push_str("-- module #");
    push_number(output, slot);
    output.push_str(": ");
    let mut rest = relative_path;
    while let Some(at) = rest.find(['\n', '\r']) {
        let (before, after) = rest.split_at(at);
        output.extend([before, if after.starts_with('\n') { "\\n" } else { "\\r" }]);
        rest = &after[1..];
    }
    output.extend([rest, "\n"]);
}

/// What opens a wrapped body.
fn write_wrapper_opening(binding: Binding, prefix: &str, output: &mut String) {
    match binding {
        Binding::Slot(slot) => {
            output.extend([prefix, "modules["]);
            push_number(output, slot);
            output.push(']');
        }
        Binding::Entry => output.extend(["local ", prefix, "entry"]),
    }
    output.push_str("=function(...)");
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rustc_hash::FxHashMap;

    use super::*;
    use crate::discovery::{ParsedModule, parse_module};
    use crate::dynamic_require::DynamicRequire;
    use crate::module::{Dependency, ModuleId};

    struct ModuleFixture<'ast> {
        info: ModuleInfo<'ast>,
        resolved_paths: Vec<PathBuf>,
    }

    fn module<'ast>(
        allocator: &'ast Allocator,
        path: &str,
        source: &str,
        resolutions: &[(&str, &str)],
    ) -> ModuleFixture<'ast> {
        let ParsedModule { mut info, requires, errors, .. } = parse_module(
            allocator,
            PathBuf::from(path),
            path.to_owned(),
            allocator.alloc_str(source).as_bytes(),
            DynamicRequire::default(),
        );
        assert!(errors.is_empty(), "fixture must parse: {errors:?}");
        let mut resolved_paths = Vec::new();
        info.dependencies = requires
            .into_iter()
            .map(|require| {
                let resolved = resolutions
                    .iter()
                    .find(|(require_string, _)| *require_string == require.require_string)
                    .unwrap_or_else(|| panic!("no resolution for {:?}", require.require_string));
                resolved_paths.push(PathBuf::from(resolved.1));
                Dependency {
                    require_string: require.require_string,
                    target: ModuleId(usize::MAX),
                    call_span: require.call_span,
                }
            })
            .collect();
        ModuleFixture { info, resolved_paths }
    }

    /// The graph of `fixtures`, given in topological order with the last one
    /// as entry, laid out as `DependencyGraph::build` lays it out, entry first.
    fn graph(mut fixtures: Vec<ModuleFixture<'_>>) -> DependencyGraph<'_> {
        fixtures.rotate_right(1);
        let ids_by_path: FxHashMap<PathBuf, ModuleId> = fixtures
            .iter()
            .enumerate()
            .map(|(i, fixture)| (fixture.info.path.clone(), ModuleId(i)))
            .collect();
        let modules: Vec<ModuleInfo<'_>> = fixtures
            .into_iter()
            .map(|mut fixture| {
                for (dependency, path) in
                    fixture.info.dependencies.iter_mut().zip(&fixture.resolved_paths)
                {
                    dependency.target = ids_by_path[path];
                }
                fixture.info
            })
            .collect();
        let topological_order = (1..modules.len()).chain([0]).map(ModuleId).collect();
        DependencyGraph { modules, topological_order, entry_id: ModuleId(0), warnings: vec![] }
    }

    fn emitted(
        allocator: &Allocator,
        fixtures: Vec<ModuleFixture<'_>>,
    ) -> (String, Vec<LineMapEntry>) {
        let (output, line_map) = emit(allocator, &mut graph(fixtures))
            .unwrap_or_else(|errors| panic!("bundle failed: {errors:?}"));
        (String::from_utf8(output).unwrap(), line_map)
    }

    fn bundle(allocator: &Allocator, fixtures: Vec<ModuleFixture<'_>>) -> String {
        emitted(allocator, fixtures).0
    }

    fn assert_reparses(output: &str) {
        let allocator = Allocator::new();
        let result = luck_parser::parse(&allocator, output.as_bytes());
        assert!(result.errors.is_empty(), "bundle must reparse: {:?}\n{output}", result.errors);
    }

    /// A module that requires `./dep` once, and the `src/dep.luau` it
    /// reaches.
    fn with_dependency<'ast>(allocator: &'ast Allocator, source: &str) -> Vec<ModuleFixture<'ast>> {
        vec![
            module(allocator, "src/dep.luau", "return 1\n", &[]),
            module(allocator, "src/main.luau", source, &[("./dep", "src/dep.luau")]),
        ]
    }

    #[test]
    fn basic_bundle_uses_lazy_loader() {
        let allocator = Allocator::new();
        let modules = vec![
            module(
                &allocator,
                "src/utils.luau",
                "\
local M = {}
return M
",
                &[],
            ),
            module(
                &allocator,
                "src/main.luau",
                "\
local utils = require(\"./utils\")
print(utils)
",
                &[("./utils", "src/utils.luau")],
            ),
        ];
        let output = bundle(&allocator, modules);
        assert!(output.contains("__luck_modules[1]=function(...)"), "{output}");
        assert!(output.contains("local utils = __luck_require(1)"), "{output}");
        assert!(output.contains("-- module #1: src/utils.luau"), "{output}");
        assert!(output.contains("local __luck_entry=function(...)"), "{output}");
        assert!(output.contains("return __luck_entry(...)"), "{output}");
        assert_reparses(&output);
    }

    #[test]
    fn many_modules_use_table_slots_not_locals() {
        let allocator = Allocator::new();
        // One `local` per module would break past the 200-locals-per-function
        // limit at ~199 modules.
        let mut modules = Vec::new();
        let mut entry_source = String::new();
        let mut resolutions: Vec<(String, String)> = Vec::new();
        for i in 0..500 {
            modules.push(module(
                &allocator,
                &format!("src/mod_{i}.luau"),
                &format!("return {i}\n"),
                &[],
            ));
            let _ = writeln!(entry_source, "require(\"./mod_{i}\")");
            resolutions.push((format!("./mod_{i}"), format!("src/mod_{i}.luau")));
        }
        let borrowed_resolutions: Vec<(&str, &str)> = resolutions
            .iter()
            .map(|(require_string, path)| (require_string.as_str(), path.as_str()))
            .collect();
        modules.push(module(&allocator, "src/main.luau", &entry_source, &borrowed_resolutions));
        let output = bundle(&allocator, modules);
        assert!(output.contains("__luck_modules[500]=function(...)"), "all modules registered");
        assert!(!output.contains("local __luck_src_mod"), "no per-module locals");
        assert_reparses(&output);
    }

    #[test]
    fn required_entry_registers_under_its_slot() {
        let allocator = Allocator::new();
        // The entry function registers under the slot after every other
        // module's, so a module requiring the entry leaves no raw require()
        // in the output. The extra load runs a fresh instance, as when the
        // main file is required a second time as a module.
        let modules = vec![
            module(
                &allocator,
                "src/b.luau",
                "\
local a = require(\"./a\")
return { a = a }
",
                &[("./a", "src/a.luau")],
            ),
            module(
                &allocator,
                "src/a.luau",
                "\
local b = require(\"./b\")
return { b = b }
",
                &[("./b", "src/b.luau")],
            ),
        ];
        let output = bundle(&allocator, modules);
        assert!(output.contains("__luck_modules[2]=__luck_entry"), "{output}");
        assert!(!output.contains("require(\"./a\")"), "no raw require may survive: {output}");
        assert!(output.contains("local a = __luck_require(2)"), "{output}");
        assert_reparses(&output);
    }

    #[test]
    fn two_require_strings_for_one_file_share_its_slot() {
        let allocator = Allocator::new();
        // Luau caches by resolved file, so two spellings of one module load
        // it once.
        let source = "\
local a = require(\"./foo\")
local b = require(\"../src/foo\")
print(a, b)
";
        let modules = vec![
            module(&allocator, "src/foo.luau", "return {}\n", &[]),
            module(
                &allocator,
                "src/main.luau",
                source,
                &[("./foo", "src/foo.luau"), ("../src/foo", "src/foo.luau")],
            ),
        ];
        let output = bundle(&allocator, modules);
        assert_eq!(output.matches("__luck_require(1)").count(), 2, "{output}");
        assert_eq!(output.matches("=function(...)").count(), 2, "{output}");
        assert_reparses(&output);
    }

    #[test]
    fn generated_prefix_avoids_user_identifiers() {
        let allocator = Allocator::new();
        let source = "\
local __luck_require = 1
local d = require(\"./dep\")
print(__luck_require, d)
";
        let output = bundle(&allocator, with_dependency(&allocator, source));
        assert!(
            output.contains("local function __luck1_require"),
            "loader must rename itself: {output}"
        );
        assert!(output.contains("local d = __luck1_require(1)"), "{output}");
        assert!(output.contains("local __luck_require = 1"), "user identifier untouched: {output}");
        assert_reparses(&output);
    }

    #[test]
    fn multiline_require_preserves_line_count() {
        let allocator = Allocator::new();
        let source = "\
local d = require(
\"./dep\"
)
print(d)
";
        let (output, line_map) = emitted(&allocator, with_dependency(&allocator, source));
        assert!(
            output.contains(
                "\
local d = __luck_require(1

)"
            ),
            "padding keeps following lines in place: {output}"
        );
        let entry = find_line_entry(&line_map, "src/main.luau");
        let output_lines: Vec<&str> = output.lines().collect();
        // Source line 4 is print(d); it must land exactly 3 lines below
        // the entry body start despite the rewritten multiline call.
        assert_eq!(output_lines[entry.bundle_start_line - 1 + 3], "print(d)");
        assert_reparses(&output);
    }

    #[test]
    fn entry_shebang_moves_up_and_keeps_its_line_for_the_map() {
        let allocator = Allocator::new();
        let source = "\
#!/usr/bin/env luau
local d = require(\"./dep\")
print(d)
";
        let (output, line_map) = emitted(&allocator, with_dependency(&allocator, source));
        assert!(
            output.starts_with(
                "\
#!/usr/bin/env luau
local __luck_modules={}
"
            ),
            "{output}"
        );
        assert_eq!(output.matches("#!").count(), 1, "{output}");
        let entry = find_line_entry(&line_map, "src/main.luau");
        let output_lines: Vec<&str> = output.lines().collect();
        assert_eq!(output_lines[entry.bundle_start_line - 1], "");
        assert_eq!(output_lines[entry.bundle_start_line - 1 + 2], "print(d)");
        assert_reparses(&output);
    }

    #[test]
    fn loader_checks_single_return_and_uses_numeric_slots() {
        let allocator = Allocator::new();
        let modules = vec![
            module(&allocator, "src/dep.luau", "return {}\n", &[]),
            module(
                &allocator,
                "src/main.luau",
                "\
local m = require(\"./dep\") :: any
print(m)
",
                &[("./dep", "src/dep.luau")],
            ),
        ];
        let output = bundle(&allocator, modules);
        assert!(output.contains("local m = __luck_require(1) :: any"), "{output}");
        assert!(
            output.contains("select(\"#\",...)~=1"),
            "modules must return exactly one value: {output}"
        );
        assert!(!output.contains("package.loaded"), "Luau has no package table: {output}");
        assert_reparses(&output);
    }

    #[test]
    fn shadowed_require_call_survives_unrewritten() {
        let allocator = Allocator::new();
        let source = "\
local d = require(\"./dep\")
local function f()
    local require = print
    require(\"./dep\")
end
f()
print(d)
";
        let output = bundle(&allocator, with_dependency(&allocator, source));
        assert!(output.contains("local d = __luck_require(1)"), "{output}");
        assert!(
            output.contains("    require(\"./dep\")"),
            "shadowed call keeps user semantics: {output}"
        );
        assert_reparses(&output);
    }

    #[test]
    fn value_exports_are_lowered_inside_bundle_wrappers() {
        let allocator = Allocator::new();
        let dependency = "\
export type Public = integer
export local value = 129312i
export const LIMIT = 0xffffffffffffffffi
@native
export function get(): integer
    return value
end
";
        let modules = vec![
            module(&allocator, "src/dep.luau", dependency, &[]),
            module(
                &allocator,
                "src/main.luau",
                "\
local dep = require(\"./dep\")
print(dep.get())
",
                &[("./dep", "src/dep.luau")],
            ),
        ];
        let output = bundle(&allocator, modules);

        assert!(!output.contains("export "), "{output}");
        assert!(output.contains("local value = 129312i"), "{output}");
        assert!(output.contains("const LIMIT = 0xffffffffffffffffi"), "{output}");
        assert!(
            output.contains(
                "\
@native
local function get()"
            ),
            "{output}"
        );
        assert!(
            output.contains("return __luck_freeze({value=value,LIMIT=LIMIT,get=get})"),
            "{output}"
        );
        assert_reparses(&output);
    }

    #[test]
    fn nested_and_field_access_requires_are_rewritten() {
        let allocator = Allocator::new();
        let source = "\
local field = require(\"./dep\").value
local function lazy()
    return require(\"./dep\")
end
if true then
    local inner = require(\"./dep\")
    print(inner)
end
print(field, lazy())
";
        let modules = vec![
            module(&allocator, "src/dep.luau", "return { value = 1 }\n", &[]),
            module(&allocator, "src/main.luau", source, &[("./dep", "src/dep.luau")]),
        ];
        let output = bundle(&allocator, modules);
        assert!(output.contains("local field = __luck_require(1).value"), "{output}");
        assert!(output.contains("return __luck_require(1)"), "{output}");
        assert!(output.contains("local inner = __luck_require(1)"), "{output}");
        assert!(!output.contains("require(\"./dep\")"), "no raw require may survive: {output}");
        assert_reparses(&output);
    }

    #[test]
    fn require_inside_string_literal_is_untouched() {
        let allocator = Allocator::new();
        let source = "\
local d = require(\"./dep\")
local msg = 'use require(\"./dep\") to load'
print(d, msg)
";
        let output = bundle(&allocator, with_dependency(&allocator, source));
        assert!(output.contains("local d = __luck_require(1)"), "{output}");
        assert!(
            output.contains("require(\"./dep\") to load"),
            "string literal corrupted: {output}"
        );
    }

    #[test]
    fn escaped_require_string_is_rewritten_as_decoded() {
        let allocator = Allocator::new();
        // require("./a\46b") is require("./a.b"), so both spellings load one
        // module.
        let source = "\
local x = require(\"./a\\46b\")
local y = require(\"./a.b\")
print(x, y)
";
        let modules = vec![
            module(&allocator, "src/a.b.luau", "return {}\n", &[]),
            module(&allocator, "src/main.luau", source, &[("./a.b", "src/a.b.luau")]),
        ];
        let output = bundle(&allocator, modules);
        assert_eq!(output.matches("__luck_require(1)").count(), 2, "{output}");
        assert!(output.contains("__luck_modules[1]=function(...)"), "{output}");
        assert_reparses(&output);
    }

    fn find_line_entry<'map>(line_map: &'map [LineMapEntry], path: &str) -> &'map LineMapEntry {
        line_map
            .iter()
            .find(|entry| entry.path == Path::new(path))
            .unwrap_or_else(|| panic!("no line map entry for {path}"))
    }

    #[test]
    fn line_map_maps_module_lines_to_original_source_lines() {
        let allocator = Allocator::new();
        let utils_source = "\
local M = {}
function M.foo()
    return 42
end
return M
";
        let entry_source = "\
local utils = require(\"./utils\")
local helper = require(\"./helper\")
print(utils, helper)
";
        // Leading comments and a blank line make the offsets nontrivial.
        let helper_source = "\
-- helper module
-- second comment line

local helper = { ready = true }
return helper
";
        let modules = vec![
            module(&allocator, "src/utils.luau", utils_source, &[]),
            module(&allocator, "src/helper.luau", helper_source, &[]),
            module(
                &allocator,
                "src/main.luau",
                entry_source,
                &[("./utils", "src/utils.luau"), ("./helper", "src/helper.luau")],
            ),
        ];
        let (output, line_map) = emitted(&allocator, modules);
        let output_lines: Vec<&str> = output.lines().collect();

        let utils = find_line_entry(&line_map, "src/utils.luau");
        // Source line 1 lands on bundle_start_line; the module has 5 lines.
        assert_eq!(output_lines[utils.bundle_start_line - 1], "local M = {}");
        assert_eq!(output_lines[utils.bundle_start_line - 1 + 4], "return M");
        assert_eq!(
            utils.bundle_end_line - utils.bundle_start_line + 1,
            5,
            "utils body spans all 5 source lines"
        );
        // The two lines directly above a module body are its provenance
        // marker and the loader-function opener.
        assert!(output_lines[utils.bundle_start_line - 2].contains("=function(...)"), "{output}");
        assert!(output_lines[utils.bundle_start_line - 3].starts_with("-- module "), "{output}");

        let helper = find_line_entry(&line_map, "src/helper.luau");
        // Nontrivial offset: the real declaration is on source line 4.
        assert_eq!(output_lines[helper.bundle_start_line - 1], "-- helper module");
        assert_eq!(output_lines[helper.bundle_start_line - 1 + 2], "");
        assert_eq!(
            output_lines[helper.bundle_start_line - 1 + 3],
            "local helper = { ready = true }"
        );
        assert_eq!(helper.bundle_end_line - helper.bundle_start_line + 1, 5);

        let entry = find_line_entry(&line_map, "src/main.luau");
        // The entry body sits inside its wrapper function; line 1 is
        // still its first source line.
        assert!(
            output_lines[entry.bundle_start_line - 1].contains("__luck_require"),
            "entry require rewritten: {output}"
        );
        assert!(
            output_lines[entry.bundle_start_line - 2].contains("__luck_entry=function(...)"),
            "{output}"
        );
        assert_eq!(output_lines[entry.bundle_start_line - 1 + 2], "print(utils, helper)");
        assert_eq!(entry.bundle_end_line - entry.bundle_start_line + 1, 3);

        // Ranges never overlap and are ordered utils, helper, entry.
        assert_eq!(line_map.len(), 3);
        assert!(utils.bundle_end_line < helper.bundle_start_line);
        assert!(helper.bundle_end_line < entry.bundle_start_line);
        assert_reparses(&output);
    }

    #[test]
    fn lone_module_emits_verbatim_and_maps_every_line() {
        let allocator = Allocator::new();
        let source = "\
print(\"a\")
print(\"b\")
print(\"c\")
";
        let modules = vec![module(&allocator, "src/main.luau", source, &[])];
        let (output, line_map) = emitted(&allocator, modules);
        assert_eq!(output, source);
        assert_eq!(
            line_map,
            [LineMapEntry {
                bundle_start_line: 1,
                bundle_end_line: 3,
                path: PathBuf::from("src/main.luau"),
            }]
        );
    }

    #[test]
    fn line_break_in_a_module_path_stays_inside_the_provenance_comment() {
        let allocator = Allocator::new();
        let path = "src/a\nerror(1)--.luau";
        let modules = vec![
            module(&allocator, path, "return 1\n", &[]),
            module(
                &allocator,
                "src/main.luau",
                "\
local d = require(\"./dep\")
print(d)
",
                &[("./dep", path)],
            ),
        ];
        let output = bundle(&allocator, modules);
        assert!(
            !output.lines().any(|line| line.starts_with("error(1)")),
            "a module path must not escape its comment:\n{output}"
        );
    }

    #[test]
    fn long_bracket_require_is_rewritten() {
        let allocator = Allocator::new();
        let source = "\
local d = require [[./dep]]
print(d)
";
        let output = bundle(&allocator, with_dependency(&allocator, source));
        assert!(output.contains("local d = __luck_require(1)"), "{output}");
        assert!(!output.contains("require [[./dep]]"), "{output}");
    }
}
