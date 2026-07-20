//! `luck lsp` - run the language server over stdio, or TCP with `--socket`.

use crate::{EXIT_FAILURE, EXIT_SUCCESS};
use clap::Args;
use std::process::ExitCode;

#[derive(Args)]
pub(crate) struct LspArgs {
    /// Bind 127.0.0.1:<port> and accept one client instead of stdio.
    #[arg(long)]
    socket: Option<u16>,
}

impl LspArgs {
    pub(crate) fn run(self) -> ExitCode {
        // Match the CLI's existing 16 MB worker-stack budget for deep parses.
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(16 * 1024 * 1024)
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("luck lsp: failed to start async runtime: {error}");
                return ExitCode::from(EXIT_FAILURE);
            }
        };
        runtime.block_on(async {
            match self.socket {
                Some(port) => {
                    if let Err(error) = luck_lsp::serve_socket(port).await {
                        eprintln!("luck lsp: socket transport failed: {error}");
                        return ExitCode::from(EXIT_FAILURE);
                    }
                    ExitCode::from(EXIT_SUCCESS)
                }
                None => {
                    luck_lsp::serve_stdio().await;
                    ExitCode::from(EXIT_SUCCESS)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::args::{Cli, Command};
    use clap::Parser;

    #[test]
    fn lsp_subcommand_parses() {
        let cli = Cli::try_parse_from(["luck", "lsp"]).expect("lsp parses");
        assert!(matches!(
            cli.command,
            Command::Lsp(super::LspArgs { socket: None })
        ));
        let with_socket = Cli::try_parse_from(["luck", "lsp", "--socket", "9000"]).unwrap();
        assert!(matches!(
            with_socket.command,
            Command::Lsp(super::LspArgs { socket: Some(9000) })
        ));
    }
}
