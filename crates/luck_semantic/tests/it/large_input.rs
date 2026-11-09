//! Very large inputs. A stack overflow aborts the whole process, so these
//! run the analysis in a child of this test binary.

use std::env;
use std::process::Command;
use std::thread::Builder;

use luck_token::LuaVersion;

const CHILD_MARKER: &str = "LUCK_SEMANTIC_BUG_LARGE_INPUT_CHILD";

/// The stack `luck_cli` runs its worker on.
const CLI_STACK_BYTES: usize = 16 * 1024 * 1024;

#[test]
fn bug_analyze_survives_long_left_associative_chain() {
    if env::var_os(CHILD_MARKER).is_none() {
        let status = Command::new(env::current_exe().expect("test binary path"))
            .args([
                "--exact",
                "large_input::bug_analyze_survives_long_left_associative_chain",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            .status()
            .expect("run the analysis child");
        assert!(
            status.success(),
            "analyze must not overflow the stack on `1 + 1 + ...`, which stock Lua compiles \
             at any length and the parser builds without recursing: {status}"
        );
        return;
    }

    let worker = Builder::new().stack_size(CLI_STACK_BYTES).spawn(|| {
        let source = format!("local x = 1{}", " + 1".repeat(100_000));
        // Leaked because dropping the left-deep tree recurses too, which isn't
        // under test.
        let parsed = Box::leak(Box::new(luck_parser::parse(&source, LuaVersion::Lua54)));
        assert!(parsed.errors.is_empty(), "parse errors: {:?}", parsed.errors);
        let analysis = luck_semantic::analyze(&parsed.block, LuaVersion::Lua54);
        assert_eq!(analysis.scope_tree.symbols.len(), 1, "only x is declared");
    });
    worker.expect("spawn the worker").join().expect("the analysis must finish");
}
