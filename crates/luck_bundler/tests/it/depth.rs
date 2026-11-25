use luck_allocator::Allocator;
use luck_ast::{MAX_NESTING_DEPTH, block_nesting_depth};
use luck_bundler::Bundle;
use luck_diagnostics::FileDiagnostics;
use luck_testgen::with_the_smallest_stack;

use crate::common::{assert_reparses, bundle_files, located, text};

/// `1` inside `count` table constructors.
fn in_tables(count: usize) -> String {
    format!("{}1{}", "{".repeat(count), "}".repeat(count))
}

/// How many levels a module's wrapper adds.
const WRAPPER_LEVELS: usize = 2;

/// A project whose deepest construct is a run of nested tables after
/// `prefix` in `deep_file`.
struct Shape {
    name: &'static str,
    deep_file: &'static str,
    prefix: &'static str,
    other_files: &'static [(&'static str, &'static [u8])],
}

const SHAPES: &[Shape] = &[
    Shape {
        name: "module",
        deep_file: "m.luau",
        prefix: "return ",
        other_files: &[("main.luau", b"return require(\"./m\")\n")],
    },
    Shape {
        name: "entry",
        deep_file: "main.luau",
        prefix: "\
require(\"./m\")
return ",
        other_files: &[("m.luau", b"return 1\n")],
    },
];

impl Shape {
    /// The deep file's source and the bundle's result.
    fn bundle(&self, tables: usize) -> (String, Result<Bundle, Vec<FileDiagnostics>>) {
        let deep_source = format!("{}{}\n", self.prefix, in_tables(tables));
        let mut files = self.other_files.to_vec();
        files.push((self.deep_file, deep_source.as_bytes()));
        let result = bundle_files(&files, "main.luau");
        (deep_source, result)
    }
}

#[test]
fn deepest_module_that_fits_bundles_to_the_cap_for_every_wrapper() {
    with_the_smallest_stack(|| {
        for shape in SHAPES {
            // The module's own block is a level, and its tables stand on it.
            let tables = MAX_NESTING_DEPTH - WRAPPER_LEVELS - 1;
            let (_, result) = shape.bundle(tables);
            let output =
                result.unwrap_or_else(|errors| panic!("{}: {errors:?}", shape.name)).output;
            let allocator = Allocator::new();
            let reparsed = luck_parser::parse(&allocator, &output);
            assert!(reparsed.errors.is_empty(), "{}: {:?}", shape.name, reparsed.errors);
            assert_eq!(
                block_nesting_depth(&reparsed.chunk),
                MAX_NESTING_DEPTH,
                "{}: the wrapper must add exactly {WRAPPER_LEVELS} levels",
                shape.name
            );
        }
    });
}

#[test]
fn flags_e013_at_the_construct_a_wrapper_pushes_past_the_cap() {
    with_the_smallest_stack(|| {
        for shape in SHAPES {
            let tables = MAX_NESTING_DEPTH - WRAPPER_LEVELS;
            let (deep_source, result) = shape.bundle(tables);
            let errors = result.err().unwrap_or_else(|| panic!("{}: must not bundle", shape.name));
            let located = located(&errors);
            let [(path, error)] = located.as_slice() else {
                panic!("{}: expected one error: {errors:?}", shape.name);
            };
            assert_eq!(error.code, "E013", "{}: {error:?}", shape.name);
            assert!(path.ends_with(shape.deep_file), "{}: {error:?}", shape.name);
            // The innermost table is the one past the cap.
            let innermost = shape.prefix.len() + tables - 1;
            let span = error.span.expect("E013 points at the construct");
            assert_eq!(span.start as usize, innermost, "{}: {error:?}", shape.name);
            assert_eq!(span.text(deep_source.as_bytes()), b"{1}", "{}: {error:?}", shape.name);
            let help = error.help.as_deref().expect("E013 has help");
            assert!(
                help.contains(&format!("{WRAPPER_LEVELS} levels deeper")),
                "{}: {help}",
                shape.name
            );
        }
    });
}

#[test]
fn flags_e013_in_every_module_past_the_cap() {
    with_the_smallest_stack(|| {
        let deep = format!("return {}\n", in_tables(MAX_NESTING_DEPTH - 1));
        let errors = bundle_files(
            &[
                (
                    "main.luau",
                    b"\
require(\"./a\")
require(\"./b\")
",
                ),
                ("a.luau", deep.as_bytes()),
                ("b.luau", deep.as_bytes()),
            ],
            "main.luau",
        )
        .expect_err("neither module fits");
        let located = located(&errors);
        let mut files: Vec<&str> = located
            .iter()
            .filter(|(_, error)| error.code == "E013")
            .filter_map(|(path, _)| path.rsplit('/').next())
            .collect();
        files.sort_unstable();
        assert_eq!(files, ["a.luau", "b.luau"], "{errors:?}");
    });
}

#[test]
fn lone_module_at_the_cap_bundles_verbatim() {
    with_the_smallest_stack(|| {
        let source = format!("return {}\n", in_tables(MAX_NESTING_DEPTH - 1));
        let output = bundle_files(&[("main.luau", source.as_bytes())], "main.luau")
            .expect("a lone module needs no wrapper")
            .output;
        assert_eq!(output, source.as_bytes());
    });
}

/// A module that exports `value` from the export table, since a closure
/// writes it, and whose deepest construct is `deep` inside `tables` table
/// constructors.
fn table_export_module(tables: usize, deep: &str) -> String {
    format!(
        "\
export local value = 0
local function bump() value += 1 end
bump()
local deep = {}{deep}{}
print(deep)
",
        "{".repeat(tables),
        "}".repeat(tables),
    )
}

#[test]
fn export_reads_are_measured_as_lowered() {
    with_the_smallest_stack(|| {
        let bundle_export_module = |source: &str| {
            bundle_files(
                &[
                    (
                        "main.luau",
                        b"\
local x = require(\"./x\")
print(x)
",
                    ),
                    ("x.luau", source.as_bytes()),
                ],
                "main.luau",
            )
        };

        let literal = table_export_module(MAX_NESTING_DEPTH - 3, "1");
        let output = bundle_export_module(&literal).expect("a literal at the cap fits").output;
        assert!(text(&output).contains("__luck_exports.value"), "{}", text(&output));
        assert_reparses(&output);

        // Lowering reads `value` as a field of the export table, one level deeper.
        let export_read = table_export_module(MAX_NESTING_DEPTH - 3, "value");
        let errors =
            bundle_export_module(&export_read).expect_err("the lowered read passes the cap");
        let located = located(&errors);
        let [(_, error)] = located.as_slice() else {
            panic!("expected one error: {errors:?}");
        };
        assert_eq!(error.code, "E013", "{error:?}");
        assert_eq!(
            error.span.map(|span| span.text(export_read.as_bytes())),
            Some(&b"value"[..]),
            "{error:?}"
        );

        let shallower_read = table_export_module(MAX_NESTING_DEPTH - 4, "value");
        bundle_export_module(&shallower_read).expect("one level shallower fits");
    });
}
