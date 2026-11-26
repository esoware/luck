use std::fs;
use std::path::Path;

/// Analysis and what it links: Ast, Common, and Config, and the Compiler,
/// Bytecode, and VM that run type functions.
const LIBRARIES: [&str; 7] = ["Analysis", "Ast", "Bytecode", "Common", "Compiler", "Config", "VM"];

fn main() {
    let luau = Path::new("luau");
    assert!(
        luau.join("Analysis").is_dir(),
        "the Luau submodule is missing, so run `git submodule update --init`"
    );

    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").warnings(false).debug(false).file("src/shim.cpp");
    if build.get_compiler().is_like_msvc() {
        build.flag("/EHsc");
    }
    println!("cargo::rerun-if-changed=src/shim.cpp");
    for library in LIBRARIES {
        let directory = luau.join(library);
        build.include(directory.join("include"));
        let sources = fs::read_dir(directory.join("src")).expect("a Luau library has sources");
        for entry in sources {
            let path = entry.expect("a Luau source directory lists").path();
            if path.extension().is_some_and(|extension| extension == "cpp") {
                build.file(path);
            }
        }
        println!("cargo::rerun-if-changed={}", directory.display());
    }
    build.compile("luau");
}
