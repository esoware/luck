use std::process::ExitCode;

#[cfg(not(any(target_arch = "arm", target_arch = "riscv64", target_family = "wasm")))]
#[global_allocator]
static GLOBAL: mimalloc_safe::MiMalloc = mimalloc_safe::MiMalloc;

fn main() -> ExitCode {
    // Larger stack for deeply nested AST processing in the minifier transforms.
    let thread = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(luck_cli::run)
        .expect("failed to spawn main thread");

    // Propagate the worker's exit code; a panic there surfaces as a failure.
    match thread.join() {
        Ok(code) => code,
        Err(_) => ExitCode::from(luck_cli::EXIT_FAILURE),
    }
}
