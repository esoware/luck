//! `luck lsp` - run the language server over stdio, or TCP with `--socket`.

use clap::Args;
use tokio::runtime::Builder;

use crate::WORKER_STACK_BYTES;
use crate::exit::Exit;

#[derive(Args)]
pub(crate) struct LspArgs {
    /// Listen on 127.0.0.1 at this port and serve one client instead of stdio.
    #[arg(long)]
    socket: Option<u16>,
}

impl LspArgs {
    /// # Errors
    ///
    /// Reports the failure and returns [`Exit::Failure`] when the async
    /// runtime can't start or the socket transport fails.
    pub(crate) fn run(self) -> Result<(), Exit> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(WORKER_STACK_BYTES)
            .build()
            .map_err(|error| {
                eprintln!("luck lsp: failed to start async runtime: {error}");
                Exit::Failure
            })?;
        runtime.block_on(async {
            if let Some(port) = self.socket {
                luck_lsp::serve_socket(port).await.map_err(|error| {
                    eprintln!("luck lsp: socket transport failed: {error}");
                    Exit::Failure
                })
            } else {
                luck_lsp::serve_stdio().await;
                Ok(())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::LspArgs;
    use crate::cli::{Cli, Command};

    #[test]
    fn lsp_subcommand_parses() {
        let cli = Cli::try_parse_from(["luck", "lsp"]).expect("lsp parses");
        assert!(matches!(cli.command, Command::Lsp(LspArgs { socket: None })));
        let with_socket = Cli::try_parse_from(["luck", "lsp", "--socket", "9000"]).unwrap();
        assert!(matches!(with_socket.command, Command::Lsp(LspArgs { socket: Some(9000) })));
    }
}
