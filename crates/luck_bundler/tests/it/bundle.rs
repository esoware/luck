use std::path::PathBuf;
use std::{fs, io};

use luck_allocator::ThreadAllocators;
use luck_bundler::{Bundle, BundleOptions, DependencyGraph, DynamicRequire, ModuleId};
use luck_diagnostics::FileDiagnostics;
use luck_resolver::{FileSystem as _, FileSystemOs, to_slash_lossy};
use tempfile::TempDir;

use crate::common::{
    REQUIRE_UTIL, RETURN_ONE, assert_reparses, bundle_both, bundle_files, bundle_project, codes,
    located, off_pool, on_pool, text, write_project,
};

fn fixture_directory(name: &str) -> PathBuf {
    let manifest_directory = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_directory).join("../../tests/fixtures").join(name).join("input")
}

fn run_bundle(name: &str, entry_name: &str) -> Result<String, Vec<FileDiagnostics>> {
    run_bundle_with(name, entry_name, DynamicRequire::default())
        .map(|result| String::from_utf8(result.output).unwrap())
}

fn run_bundle_with(
    name: &str,
    entry_name: &str,
    dynamic_require: DynamicRequire,
) -> Result<Bundle, Vec<FileDiagnostics>> {
    let input_directory = fixture_directory(name);
    let entry = input_directory.join(entry_name);
    bundle_both(&entry, &BundleOptions { project_root: &input_directory, dynamic_require })
}

#[test]
fn basic_bundle() {
    let output = run_bundle("basic_bundle", "main.lua").expect("bundle failed");
    insta::assert_snapshot!("basic_bundle", output);
}

#[test]
fn nested_deps() {
    let output = run_bundle("nested_deps", "main.lua").expect("bundle failed");
    insta::assert_snapshot!("nested_deps", output);
}

#[test]
fn diamond_deps() {
    let output = run_bundle("diamond_deps", "main.lua").expect("bundle failed");
    insta::assert_snapshot!("diamond_deps", output);
}

#[test]
fn deep_chain() {
    let output = run_bundle("deep_chain", "main.lua").expect("bundle failed");
    insta::assert_snapshot!("deep_chain", output);
}

#[test]
fn init_module() {
    let output = run_bundle("init_module", "main.lua").expect("bundle failed");
    insta::assert_snapshot!("init_module", output);
}

#[test]
fn multiple_requires() {
    let output = run_bundle("multiple_requires", "main.lua").expect("bundle failed");
    insta::assert_snapshot!("multiple_requires", output);
}

#[test]
fn module_required_by_many() {
    let output = run_bundle("module_required_by_many", "main.lua").expect("bundle failed");
    insta::assert_snapshot!("module_required_by_many", output);
}

#[test]
fn nil_return() {
    let output = run_bundle("nil_return", "main.lua").expect("bundle failed");
    insta::assert_snapshot!("nil_return", output);
}

#[test]
fn no_deps() {
    let output = run_bundle("no_deps", "main.lua").expect("bundle failed");
    insta::assert_snapshot!("no_deps", output);
}

#[test]
fn circular_dependency_bundles_with_w003() {
    // The lazy loader lets a cycle bundle; a W003 warning flags the risk.
    let result = run_bundle_with("errors/circular_dep", "a.lua", DynamicRequire::default())
        .expect("cycle must bundle");
    assert!(
        codes(&result.warnings).contains(&"W003"),
        "Expected W003 warning, got: {:?}",
        codes(&result.warnings)
    );
    assert!(text(&result.output).contains("__luck_require"));
    // The cycle runs through the entry: its function must register under
    // its slot so the bundle stays self-contained.
    assert!(
        text(&result.output).contains("__luck_modules[2]=__luck_entry"),
        "entry must register when required:\n{}",
        text(&result.output)
    );
}

#[test]
fn missing_entry_flags_e010() {
    let errors = bundle_files(&[], "main.luau").expect_err("a missing entry must not bundle");
    assert_eq!(codes(&errors), ["E010"], "{errors:?}");
}

#[test]
fn unresolved_module_flags_e004() {
    let errors = run_bundle("errors/unresolved_module", "main.lua")
        .expect_err("an unresolved module must not bundle");
    assert!(codes(&errors).contains(&"E004"), "Expected E004, got: {:?}", codes(&errors));
}

/// `something.lua` sits beside the entry, but a require without `./`,
/// `../`, or `@` names no file.
#[test]
fn bare_require_flags_e004_for_its_missing_prefix() {
    let errors = run_bundle("errors/bare_require", "main.lua")
        .expect_err("a require without a prefix must not bundle");
    let located = located(&errors);
    let [(_, error)] = located.as_slice() else {
        panic!("expected one error: {errors:?}");
    };
    assert_eq!(error.code, "E004");
    assert_eq!(
        error.message,
        "module not found: `something` (Luau requires must start with `./`, `../`, or `@`)"
    );
}

#[test]
fn non_literal_require_flags_e002_in_error_mode() {
    let errors = run_bundle_with("errors/non_literal_require", "main.lua", DynamicRequire::Error)
        .expect_err("error mode must refuse a dynamic require");
    assert!(codes(&errors).contains(&"E002"), "Expected E002, got: {:?}", codes(&errors));
}

/// The default: the bundle is produced, the call stays as written for the
/// runtime's own `require`, and W007 says so.
#[test]
fn dynamic_require_stays_for_the_runtime() {
    let result = run_bundle_with("dynamic_require", "main.lua", DynamicRequire::default())
        .expect("a dynamic require must not abort the bundle");

    assert!(
        codes(&result.warnings).contains(&"W007"),
        "Expected W007, got: {:?}",
        codes(&result.warnings)
    );
    assert!(text(&result.output).contains("require(name)"), "{}", text(&result.output));
    insta::assert_snapshot!("dynamic_require", text(&result.output));
}

#[test]
fn dynamic_require_in_allow_mode_is_silent() {
    let result = run_bundle_with("dynamic_require", "main.lua", DynamicRequire::Allow)
        .expect("bundle failed");
    assert!(result.warnings.is_empty(), "expected no warnings, got: {:?}", codes(&result.warnings));
    assert!(text(&result.output).contains("require(name)"), "{}", text(&result.output));
}

#[test]
fn side_effect_require_bundles() {
    // A require as a statement, for the module's side effects, is legal.
    let bundled = bundle_files(
        &[("main.luau", b"require(\"./util\")\nprint(\"done\")\n"), ("util.luau", RETURN_ONE)],
        "main.luau",
    )
    .expect("a side-effect require must bundle");
    let output = text(&bundled.output);
    assert!(output.contains("__luck_require(1)"), "{output}");
    assert_reparses(&bundled.output);
}

#[test]
fn require_after_code_bundles() {
    // Requires are position-independent with the lazy loader.
    let output = run_bundle("errors/require_after_code", "main.lua")
        .expect("require after code must bundle");
    assert!(output.contains("__luck_require"), "{output}");
}

#[test]
fn package_loaded_write_bundles_as_written() {
    // Luau's `require` never reads `package.loaded`, so the write means in
    // the bundle what it means without one.
    let output = run_bundle("errors/package_loaded_manip", "main.luau")
        .expect("package.loaded writes bundle fine");
    assert!(output.contains("package.loaded[\"x\"] = {}"), "{output}");
    assert!(output.contains("local x = __luck_require(1)"), "{output}");
}

#[test]
fn relative_require() {
    let output = run_bundle("relative_require", "main.luau").expect("bundle failed");
    insta::assert_snapshot!("relative_require", output);
}

#[test]
fn alias_require() {
    let output = run_bundle("alias_require", "src/main.luau").expect("bundle failed");
    insta::assert_snapshot!("alias_require", output);
}

#[test]
fn luaurc_inheritance() {
    let output =
        run_bundle("luaurc_inheritance", "src/deep/nested/mod.luau").expect("bundle failed");
    insta::assert_snapshot!("luaurc_inheritance", output);
}

#[test]
fn init_luau() {
    let output = run_bundle("init_luau", "main.luau").expect("bundle failed");
    insta::assert_snapshot!("init_luau", output);
}

#[test]
fn type_annotations() {
    let output = run_bundle("type_annotations", "main.luau").expect("bundle failed");
    insta::assert_snapshot!("type_annotations", output);
}

#[test]
fn string_interpolation() {
    let output = run_bundle("string_interpolation", "main.luau").expect("bundle failed");
    insta::assert_snapshot!("string_interpolation", output);
}

#[test]
fn ambiguous_extension_flags_e007() {
    let errors = run_bundle("errors/ambiguous_extension", "main.luau")
        .expect_err("an ambiguous module must not bundle");
    assert!(codes(&errors).contains(&"E007"), "Expected E007, got: {:?}", codes(&errors));
}

#[test]
fn entry_hot_comments_are_hoisted() {
    // Hot comments only apply before any code, so the entry module's
    // leading run must reach the very top of the bundle.
    let output = run_bundle("hot_comments", "main.luau").expect("bundle failed");
    assert!(
        output.starts_with(
            "\
--!strict
--!native
"
        ),
        "entry hot comments must lead the bundle:\n{output}"
    );
}

#[test]
fn dependency_hot_comments_warn_w006() {
    // A non-entry module's hot comments land mid-bundle where Luau
    // ignores them; W006 makes that visible.
    let result = run_bundle_with("hot_comments", "main.luau", DynamicRequire::default())
        .expect("bundle failed");
    assert!(
        located(&result.warnings).iter().any(|(path, warning)| {
            path.ends_with("util.luau")
                && warning.code == "W006"
                && warning.message.contains("--!nonstrict")
        }),
        "Expected W006 for util.luau, got: {:?}",
        codes(&result.warnings)
    );
}

#[test]
fn export_type_is_stripped_inside_module_wrappers() {
    // `export type` is invalid below the top level; bundled module
    // bodies live inside loader functions, so the keyword is dropped
    // (the alias stays usable within its module).
    let output = run_bundle("export_type", "main.luau").expect("bundle failed");
    assert!(
        !output.contains("export type"),
        "export must be stripped inside module wrappers:\n{output}"
    );
    assert!(output.contains("type Adder"), "the alias itself must survive:\n{output}");
    assert_reparses(output.as_bytes());
}

/// The bundle must name no directory on the build host. Provenance comments
/// stay project-relative, even for a module vendored above the project root.
#[test]
fn emitted_paths_stay_project_relative() {
    let directory = tempfile::tempdir().expect("tempdir");
    let source_directory = directory.path().join("src");
    let shared_directory = directory.path().join("shared");
    fs::create_dir_all(&source_directory).expect("mkdir src");
    fs::create_dir_all(&shared_directory).expect("mkdir shared");
    fs::write(
        source_directory.join("main.luau"),
        "\
local util = require(\"../shared/util\")
return util.value
",
    )
    .expect("write main");
    fs::write(shared_directory.join("util.luau"), "return { value = 1 }\n").expect("write util");

    // Rooted at the entry's own directory, as `luck bundle src/main.luau`
    // does, so `shared/` lands above the root.
    let bundled = bundle_both(
        &source_directory.join("main.luau"),
        &BundleOptions {
            project_root: &source_directory,
            dynamic_require: DynamicRequire::default(),
        },
    )
    .expect("bundle failed");
    let output = text(&bundled.output);

    let native_root = directory.path().to_string_lossy().to_string();
    let slash_root = native_root.replace('\\', "/");
    assert!(
        !output.contains(&native_root) && !output.contains(&slash_root),
        "absolute path leaked into the bundle:\n{output}"
    );
    assert!(output.contains("../shared/util.luau"), "{output}");
}

#[test]
fn entry_shebang_moves_to_the_first_line_of_the_bundle() {
    let main = format!(
        "\
#!/usr/bin/env luau
{REQUIRE_UTIL}"
    );
    let bundled =
        bundle_files(&[("main.luau", main.as_bytes()), ("util.luau", RETURN_ONE)], "main.luau")
            .expect("bundle failed");
    let output = text(&bundled.output);
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
    assert_reparses(&bundled.output);
}

#[test]
fn dependency_shebang_is_dropped() {
    let bundled = bundle_files(
        &[
            ("main.luau", REQUIRE_UTIL.as_bytes()),
            (
                "util.luau",
                b"\
#!/usr/bin/env luau
return 1
",
            ),
        ],
        "main.luau",
    )
    .expect("bundle failed");
    let output = text(&bundled.output);
    assert!(output.starts_with("local __luck_modules={}\n"), "{output}");
    assert!(!output.contains("#!"), "{output}");
    assert_reparses(&bundled.output);
}

#[test]
fn lone_module_with_a_shebang_stays_verbatim() {
    let main = b"\
#!/usr/bin/env luau
print(1)
";
    let bundled = bundle_files(&[("main.luau", main)], "main.luau").expect("bundle failed");
    assert_eq!(bundled.output, main);
}

#[test]
fn hot_comment_after_a_block_comment_or_shebang_is_hoisted() {
    for (header, opening) in [
        (
            "\
--[[
block comment
]]
",
            "\
--!strict
local __luck_modules={}
",
        ),
        (
            "--[[ one line ]]\n",
            "\
--!strict
local __luck_modules={}
",
        ),
        (
            "#!/usr/bin/env luau\n",
            "\
#!/usr/bin/env luau
--!strict
local __luck_modules={}
",
        ),
    ] {
        let main = format!(
            "\
{header}--!strict
{REQUIRE_UTIL}"
        );
        let bundled =
            bundle_files(&[("main.luau", main.as_bytes()), ("util.luau", RETURN_ONE)], "main.luau")
                .expect("bundle failed");
        let output = text(&bundled.output);
        assert!(output.starts_with(opening), "header {header:?}:\n{output}");
        assert_reparses(&bundled.output);
    }
}

#[test]
fn hot_comment_line_inside_a_block_comment_is_not_hoisted() {
    let main = format!(
        "\
--[[
--!strict
]]
{REQUIRE_UTIL}"
    );
    let bundled =
        bundle_files(&[("main.luau", main.as_bytes()), ("util.luau", RETURN_ONE)], "main.luau")
            .expect("bundle failed");
    let output = text(&bundled.output);
    assert!(output.starts_with("local __luck_modules={}\n"), "{output}");
}

#[test]
fn hot_comment_after_code_on_a_comment_line_is_not_hoisted() {
    let main = format!(
        "\
--[[x]] local y = 1
--!strict
{REQUIRE_UTIL}"
    );
    let bundled =
        bundle_files(&[("main.luau", main.as_bytes()), ("util.luau", RETURN_ONE)], "main.luau")
            .expect("bundle failed");
    let output = text(&bundled.output);
    assert!(output.starts_with("local __luck_modules={}\n"), "{output}");
}

#[test]
fn hoisted_hot_comment_drops_the_carriage_return() {
    let main = b"--!strict\r\nlocal util = require(\"./util\")\r\nprint(util)\r\n";
    let bundled = bundle_files(&[("main.luau", main), ("util.luau", RETURN_ONE)], "main.luau")
        .expect("bundle failed");
    let output = text(&bundled.output);
    assert!(output.starts_with("--!strict\nlocal __luck_modules={}\n"), "{output}");
}

#[test]
fn dependency_hot_comment_scan_matches_the_entry_scan() {
    for (util, expected_warnings) in [
        (
            "\
--[[
--!strict
]]
return 1
",
            0,
        ),
        (
            "\
--[[x]] local y = 1
--!strict
return 1
",
            0,
        ),
        (
            "\
--[[
block
]]
--!strict
return 1
",
            1,
        ),
    ] {
        let result = bundle_files(
            &[("main.luau", REQUIRE_UTIL.as_bytes()), ("util.luau", util.as_bytes())],
            "main.luau",
        )
        .expect("bundle failed");
        let w006_count = codes(&result.warnings).iter().filter(|&&code| code == "W006").count();
        assert_eq!(w006_count, expected_warnings, "util {util:?}: {:?}", result.warnings);
    }
}

/// A byte that isn't UTF-8 outside a string is one parse error, as Luau
/// reads it, which the module reports once however many modules require it.
#[test]
fn broken_dependency_reports_its_error_once_for_its_requirers() {
    let errors = bundle_files(
        &[
            (
                "main.luau",
                b"\
require(\"./a\")
require(\"./b\")
",
            ),
            ("a.luau", b"require(\"./bad\")\n"),
            ("b.luau", b"require(\"./bad\")\n"),
            ("bad.luau", &[0xFF]),
        ],
        "main.luau",
    )
    .expect_err("a parse error must not bundle");
    assert_eq!(codes(&errors), ["E008"], "{errors:?}");
}

#[test]
fn broken_dependency_required_twice_by_one_file_reports_its_error_once() {
    let errors = bundle_files(
        &[
            (
                "main.luau",
                b"\
require(\"./bad\")
require(\"./bad\")
",
            ),
            ("bad.luau", &[0xFF]),
        ],
        "main.luau",
    )
    .expect_err("a parse error must not bundle");
    assert_eq!(codes(&errors), ["E008"], "{errors:?}");
}

/// The modules' bytes reach the bundle as written, a string or a comment
/// that isn't UTF-8 included.
#[test]
fn bytes_that_are_not_utf8_reach_the_bundle_as_written() {
    let output = bundle_files(
        &[
            ("main.luau", b"-- caf\xE9\nreturn require(\"./util\")\n"),
            ("util.luau", b"return '\xFF\xE9'\n"),
        ],
        "main.luau",
    )
    .expect("bundle")
    .output;
    for written in [&b"-- caf\xE9\n"[..], b"return '\xFF\xE9'\n"] {
        assert!(output.windows(written.len()).any(|window| window == written), "{written:?}");
    }
}

#[test]
fn require_that_resolves_nowhere_flags_e004_in_each_module_making_it() {
    let errors = bundle_files(
        &[
            (
                "main.luau",
                b"require(\"./a\")
require(\"./b\")
",
            ),
            ("a.luau", b"require(\"./missing\")\n"),
            ("b.luau", b"require(\"./missing\")\n"),
        ],
        "main.luau",
    )
    .expect_err("an unresolved module must not bundle");
    assert_eq!(codes(&errors), ["E004", "E004"], "{errors:?}");
    let files: Vec<&str> =
        errors.iter().filter_map(|file| file.path.file_name()?.to_str()).collect();
    assert_eq!(files, ["a.luau", "b.luau"], "{errors:?}");
}

/// The canonical form of `directory`, as diagnostics write it.
fn slashed_root(directory: &TempDir) -> String {
    let canonical = FileSystemOs.canonicalize(directory.path()).expect("canonicalize");
    to_slash_lossy(&canonical).into_owned()
}

#[test]
fn unresolved_require_lists_the_four_files_it_could_be() {
    let directory = write_project(&[("main.luau", b"require(\"./missing\")\n")]);
    let errors = bundle_project(directory.path(), "main.luau")
        .expect_err("an unresolved module must not bundle");
    let located = located(&errors);
    let [(path, error)] = located.as_slice() else {
        panic!("expected one error: {errors:?}");
    };
    let root = slashed_root(&directory);
    let base = format!("{root}/./missing");
    assert_eq!(error.code, "E004");
    assert_eq!(*path, format!("{root}/main.luau"));
    assert_eq!(error.message, "module not found: `./missing`");
    assert_eq!(
        error.help.as_deref(),
        Some(&*format!(
            "\
searched paths:
  - `{base}.luau`
  - `{base}.lua`
  - `{base}/init.luau`
  - `{base}/init.lua`"
        ))
    );
}

#[test]
fn ambiguous_require_names_both_files() {
    let directory = write_project(&[
        ("main.luau", b"require(\"./util\")\n"),
        ("util.luau", RETURN_ONE),
        ("util.lua", RETURN_ONE),
    ]);
    let errors = bundle_project(directory.path(), "main.luau")
        .expect_err("an ambiguous module must not bundle");
    let located = located(&errors);
    let [(_, error)] = located.as_slice() else {
        panic!("expected one error: {errors:?}");
    };
    let root = slashed_root(&directory);
    assert_eq!(error.code, "E007");
    assert_eq!(error.message, "ambiguous module resolution");
    assert_eq!(
        error.help.as_deref(),
        Some(&*format!(
            "ambiguity: both `{root}/util.luau` and `{root}/util.lua` exist, so remove one of \
             the conflicting files"
        ))
    );
}

#[test]
fn luaurc_alias_named_self_warns_w004_where_self_is_required() {
    let directory = write_project(&[
        ("main.luau", b"return require(\"./pkg\")\n"),
        (".luaurc", br#"{"aliases":{"self":"./elsewhere"}}"#),
    ]);
    let package = directory.path().join("pkg");
    fs::create_dir_all(&package).expect("mkdir pkg");
    fs::write(package.join("init.luau"), "return require(\"@self/x\")\n").expect("write init");
    fs::write(package.join("x.luau"), RETURN_ONE).expect("write x");
    let warnings = bundle_project(directory.path(), "main.luau").expect("bundle failed").warnings;
    let located = located(&warnings);
    let [(path, warning)] = located.as_slice() else {
        panic!("expected one warning: {warnings:?}");
    };
    assert_eq!(warning.code, "W004");
    assert_eq!(*path, format!("{}/pkg/init.luau", slashed_root(&directory)));
}

#[test]
fn oversized_dependency_reports_e012_once_for_its_requirers() {
    let oversized = vec![b'-'; 10 * 1024 * 1024 + 1];
    let errors = bundle_files(
        &[
            (
                "main.luau",
                b"\
require(\"./a\")
require(\"./b\")
",
            ),
            ("a.luau", b"require(\"./huge\")\n"),
            ("b.luau", b"require(\"./huge\")\n"),
            ("huge.luau", &oversized),
        ],
        "main.luau",
    )
    .expect_err("an oversized file must not bundle");
    assert_eq!(codes(&errors), ["E012"], "{errors:?}");
}

/// Eight directories of twenty modules, so threads race to resolve into the
/// same directories.
///
/// Each module requires every module of its own directory, the first of the
/// next, and, when `has_missing_requires`, one that doesn't exist, while the
/// entry requires few.
fn wide_project(has_missing_requires: bool) -> io::Result<TempDir> {
    let directory = tempfile::tempdir()?;
    // `./` is the requiring module's directory, which is the root only for
    // `main`.
    let require =
        |up: &str, package: &str, module: &str| format!("require(\"{up}/{package}/{module}\")\n");
    let packages = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let modules: Vec<String> = (0..20).map(|module| format!("m{module}")).collect();
    let main: String = packages.iter().map(|package| require(".", package, "m0")).collect();
    fs::write(directory.path().join("main.luau"), main)?;
    for (package, next_package) in packages.iter().zip(packages.iter().cycle().skip(1)) {
        let package_directory = directory.path().join(package);
        fs::create_dir_all(&package_directory)?;
        for module in &modules {
            let mut source: String =
                modules.iter().map(|sibling| require("..", package, sibling)).collect();
            source.push_str(&require("..", next_package, "m0"));
            if has_missing_requires {
                source.push_str(&require("..", package, "missing"));
            }
            source.push_str("return {}\n");
            fs::write(package_directory.join(format!("{module}.luau")), source)?;
        }
    }
    Ok(directory)
}

#[test]
fn threads_sharing_one_resolver_find_what_one_thread_does() {
    for has_missing_requires in [false, true] {
        let directory = wide_project(has_missing_requires).expect("project");
        let entry = directory.path().join("main.luau");
        let options = BundleOptions {
            project_root: directory.path(),
            dynamic_require: DynamicRequire::default(),
        };
        let bundle = || luck_bundler::bundle(&ThreadAllocators::new(), &entry, &options);
        let one_thread = on_pool(1, bundle);
        match &one_thread {
            Ok(bundled) => assert_eq!(bundled.source_files.len(), 161),
            Err(errors) => assert_eq!(codes(errors), ["E004"; 160]),
        }
        assert_eq!(one_thread.is_err(), has_missing_requires);
        for _ in 0..4 {
            let threaded = on_pool(16, bundle);
            assert_eq!(format!("{threaded:?}"), format!("{one_thread:?}"));
        }
    }
}

#[test]
fn dependency_targets_index_the_modules_they_resolve_to() {
    let directory = write_project(&[
        ("main.luau", b"require(\"./a\")\n"),
        (
            "a.luau",
            b"\
require(\"./b\")
require(\"./b\")
",
        ),
        ("b.luau", RETURN_ONE),
    ]);
    let arenas = ThreadAllocators::new();
    let options = BundleOptions {
        project_root: directory.path(),
        dynamic_require: DynamicRequire::default(),
    };
    let graph =
        off_pool(|| DependencyGraph::build(&arenas, &directory.path().join("main.luau"), &options))
            .expect("graph failed");

    let relative_paths: Vec<&str> =
        graph.modules.iter().map(|module| module.relative_path.as_str()).collect();
    assert_eq!(relative_paths, ["main.luau", "a.luau", "b.luau"]);
    let targets = |id: ModuleId| -> Vec<&str> {
        graph[id]
            .dependencies
            .iter()
            .map(|dependency| graph[dependency.target].relative_path.as_str())
            .collect()
    };
    assert_eq!(targets(ModuleId(0)), ["a.luau"]);
    assert_eq!(targets(ModuleId(1)), ["b.luau", "b.luau"]);
    assert_eq!(targets(ModuleId(2)), Vec::<&str>::new());
}
