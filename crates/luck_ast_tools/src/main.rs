//! # `luck_ast_tools`
//!
//! Writes `luck_ast`'s support code from the types in it that derive `Ast`,
//! into `luck_ast/src/generated/`:
//!
//! - `visit.rs` and `visit_mut.rs`: `Visit` and `VisitMut`, with a hook for
//!   every type the tree visits.
//! - `ast_kind.rs`: `AstKind`, a reference to any node, and `AstType`.
//! - `derive_get_span.rs`: `GetSpan` for every node.
//! - `derive_content_eq.rs`: `ContentEq` for every type.
//! - `ast_builder.rs`: a constructor for every node type.
//! - `assert_layouts.rs`: a test that pins every type's size.
//!
//! `just ast` rewrites them, and `generated_code_is_up_to_date` fails while
//! one is stale. The tool reads the definitions as source text, so it runs
//! however broken `luck_ast` is. Ported from oxc's `tasks/ast_tools`.

mod arms;
mod assert_layouts;
mod ast_builder;
mod ast_kind;
mod content_eq;
mod get_span;
mod output;
mod schema;
mod visit;

use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::schema::Schema;

fn main() -> io::Result<()> {
    for (path, code) in generate()? {
        if read(&path)? != code {
            fs::write(&path, code)?;
        }
    }
    Ok(())
}

/// The path and contents of every generated file.
fn generate() -> io::Result<Vec<(PathBuf, String)>> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .ok_or_else(|| io::Error::other("the crate isn't in a workspace's `crates/`"))?;
    let source_dir = workspace.join("crates/luck_ast/src");
    let schema = Schema::load(&source_dir)?;
    let outputs = [
        ("assert_layouts.rs", assert_layouts::generate(&schema)),
        ("ast_builder.rs", ast_builder::generate(&schema)),
        ("ast_kind.rs", ast_kind::generate(&schema)),
        ("derive_content_eq.rs", content_eq::generate(&schema)),
        ("derive_get_span.rs", get_span::generate(&schema)),
        ("visit.rs", visit::generate_visit(&schema)),
        ("visit_mut.rs", visit::generate_visit_mut(&schema)),
    ];
    let generated_dir = source_dir.join("generated");
    outputs
        .into_iter()
        .map(|(name, tokens)| {
            Ok((generated_dir.join(name), output::print_rust(tokens, workspace)?))
        })
        .collect()
}

/// `path`'s contents, or nothing if there's no file.
fn read(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_code_is_up_to_date() {
        for (path, code) in generate().expect("generate") {
            let committed = read(&path).expect("read the committed file");
            assert!(
                committed == code,
                "{} is stale; regenerate it with `just ast`",
                path.display()
            );
        }
    }
}
