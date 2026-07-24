use luck_codegen::compact;
use luck_parser::parse;
use luck_token::LuaVersion;
use std::path::{Path, PathBuf};

fn detect_version(path: &Path) -> LuaVersion {
    let path_str = path.to_string_lossy();
    if path_str.contains("luau") {
        return LuaVersion::Luau;
    }
    if path_str.contains("lua51") {
        return LuaVersion::Lua51;
    }
    if path_str.contains("lua52") {
        return LuaVersion::Lua52;
    }
    if path_str.contains("lua53") {
        return LuaVersion::Lua53;
    }
    if path_str.contains("lua55") {
        return LuaVersion::Lua55;
    }
    LuaVersion::Lua54
}

fn is_error_fixture(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "errors")
}

fn collect_fixture_files() -> Vec<PathBuf> {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has parent")
        .parent()
        .expect("crates dir has parent")
        .join("tests")
        .join("fixtures");

    let mut files = Vec::new();
    walk_dir(&fixtures_dir, &mut files);
    files
}

fn walk_dir(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, files);
        } else {
            let ext = path.extension().and_then(|e| e.to_str());
            if matches!(ext, Some("lua" | "luau")) {
                files.push(path);
            }
        }
    }
}

#[test]
fn compact_roundtrip_all_fixtures() {
    let files = collect_fixture_files();
    assert!(!files.is_empty(), "no fixture files found");

    let mut failures = Vec::new();
    let mut success_count = 0;

    for path in &files {
        if is_error_fixture(path) {
            continue;
        }
        let source = std::fs::read_to_string(path).expect("failed to read fixture file");
        let version = detect_version(path);
        let result = parse(&source, version);

        if !result.errors.is_empty() {
            continue;
        }

        let compacted = compact(&result.block, &result.source);
        let reparse = parse(&compacted, version);

        if !reparse.errors.is_empty() {
            failures.push((path.clone(), compacted, reparse.errors));
        } else {
            success_count += 1;
        }
    }

    if !failures.is_empty() {
        let mut msg = format!("{} fixture(s) failed compact roundtrip:\n", failures.len());
        for (path, compacted, errors) in &failures {
            msg.push_str(&format!("  {}:\n", path.display()));
            msg.push_str(&format!("    compact output: {compacted:?}\n"));
            for error in errors {
                msg.push_str(&format!("    - {}\n", error.message));
            }
        }
        msg.push_str(&format!(
            "{success_count} fixture(s) passed compact roundtrip"
        ));
        panic!("{msg}");
    }

    assert!(
        success_count > 0,
        "compact roundtrip passed for {success_count} fixtures"
    );
}

#[test]
fn function_attributes_roundtrip_through_codegen() {
    let source = "@native function f() end";
    let result = parse(source, LuaVersion::Luau);
    assert!(
        result.errors.is_empty(),
        "parse errors: {:?}",
        result.errors
    );
    let output = compact(&result.block, &result.source);
    assert!(
        output.contains("@native"),
        "attribute dropped from output: {output:?}"
    );
    let reparsed = parse(&output, LuaVersion::Luau);
    assert!(
        reparsed.errors.is_empty(),
        "reparse errors: {:?}",
        reparsed.errors
    );
}

#[test]
fn type_function_roundtrips_through_codegen() {
    let source = "type function Pair(t)\n\treturn t\nend";
    let result = parse(source, LuaVersion::Luau);
    assert!(
        result.errors.is_empty(),
        "parse errors: {:?}",
        result.errors
    );
    let output = compact(&result.block, &result.source);
    assert!(
        output.contains("type function Pair"),
        "type function mangled: {output:?}"
    );
    assert!(
        !output.contains('='),
        "type function must not gain an '=': {output:?}"
    );
    let reparsed = parse(&output, LuaVersion::Luau);
    assert!(
        reparsed.errors.is_empty(),
        "reparse errors: {:?}",
        reparsed.errors
    );
}
