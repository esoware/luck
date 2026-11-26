use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Analysis and what it links: Ast, Common, and Config, and the Compiler,
/// Bytecode, and VM that run type functions.
const LIBRARIES: [&str; 7] = ["Analysis", "Ast", "Bytecode", "Common", "Compiler", "Config", "VM"];

fn main() {
    let luau = Path::new("luau");
    assert!(
        luau.join("Analysis").is_dir(),
        "the Luau submodule is missing, so run `git submodule update --init`"
    );

    let target = env::var("TARGET").expect("Cargo sets `TARGET` for a build script");
    let mut build = cc::Build::new();
    build.cpp(true);
    if !names_compiler(&target) {
        build.compiler(find_clang(&target));
    }
    let compiler = build.get_compiler();
    assert!(
        compiler.is_like_clang() || compiler.is_like_clang_cl(),
        "`CXX` names `{}`, which isn't clang, and luck_type_checker builds Luau with clang",
        compiler.path().display()
    );

    // cc's opt level 3 is `-O3`, or for clang-cl `-O2`, which clang-cl reads as `-O3`.
    build.std("c++17").warnings(false).debug(false).opt_level(3).define("NDEBUG", None);
    if compiler.is_like_msvc() {
        build.flag("/EHsc");
    }
    // Full LTO, over the `-flto=thin` cc gives clang++, linked a smaller binary than thin LTO and
    // checked as fast.
    if is_linker_plugin_lto() {
        build.flag("-flto");
    }
    // Luau's CMakeLists.txt passes `-fno-math-errno` to the VM alone.
    let mut vm = build.clone();
    vm.flag("-fno-math-errno");

    build.file("src/shim.cpp");
    println!("cargo::rerun-if-changed=src/shim.cpp");
    for library in LIBRARIES {
        let directory = luau.join(library);
        build.include(directory.join("include"));
        vm.include(directory.join("include"));
        let library_build = if library == "VM" { &mut vm } else { &mut build };
        let sources = fs::read_dir(directory.join("src")).expect("a Luau library has sources");
        for entry in sources {
            let path = entry.expect("a Luau source directory lists").path();
            if path.extension().is_some_and(|extension| extension == "cpp") {
                library_build.file(path);
            }
        }
        println!("cargo::rerun-if-changed={}", directory.display());
    }
    build.compile("luau");
    vm.compile("luau_vm");
}

/// Whether the environment names a C++ compiler for `target`, in one of the
/// variables cc reads.
fn names_compiler(target: &str) -> bool {
    let kind = if env::var("HOST").is_ok_and(|host| host == target) { "HOST" } else { "TARGET" };
    let keys = [
        format!("CXX_{target}"),
        format!("CXX_{}", target.replace(['-', '.'], "_")),
        format!("{kind}_CXX"),
        "CXX".to_owned(),
    ];
    let mut is_named = false;
    for key in keys {
        println!("cargo::rerun-if-env-changed={key}");
        is_named |= env::var_os(key).is_some_and(|value| !value.is_empty());
    }
    is_named
}

/// `clang-cl` for an MSVC target and `clang++` for any other, from `PATH`, or
/// on Windows from where LLVM's installer puts it, which it leaves off `PATH`.
fn find_clang(target: &str) -> PathBuf {
    let name = if target.ends_with("-msvc") { "clang-cl" } else { "clang++" };
    let file = format!("{name}{}", env::consts::EXE_SUFFIX);
    let mut directories: Vec<PathBuf> =
        env::var_os("PATH").map(|path| env::split_paths(&path).collect()).unwrap_or_default();
    let installed = env::var_os("ProgramFiles")
        .filter(|_| cfg!(windows))
        .map(|program_files| Path::new(&program_files).join("LLVM").join("bin"));
    directories.extend(installed.clone());
    let found =
        directories.into_iter().map(|directory| directory.join(&file)).find(|path| path.is_file());
    let places = installed.map_or_else(
        || "`PATH`".to_owned(),
        |directory| format!("`PATH` or in `{}`", directory.display()),
    );
    found.unwrap_or_else(|| {
        panic!(
            "luck_type_checker builds Luau with `{name}`, which isn't on {places}, so install LLVM at the major version of the LLVM `rustc -vV` prints, or name a clang in `CXX`"
        )
    })
}

/// Whether rustc leaves LTO to the linker, as `-Clinker-plugin-lto` asks, so
/// the C++ has to be LLVM bitcode for the linker to optimize with the Rust.
fn is_linker_plugin_lto() -> bool {
    let flags = env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    let mut flags = flags.split('\x1f');
    let mut options = Vec::new();
    while let Some(flag) = flags.next() {
        if flag == "-C" {
            options.extend(flags.next());
        } else if let Some(option) = flag.strip_prefix("-C") {
            options.push(option);
        }
    }
    options
        .iter()
        .rev()
        .find_map(|option| option.strip_prefix("linker-plugin-lto"))
        .is_some_and(|value| !matches!(value, "=n" | "=no" | "=off" | "=false"))
}
