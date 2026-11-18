use clap::Args;
use luck_ast::MIN_STACK_BYTES;
use rayon::ThreadPoolBuilder;

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
    /// Reports the failure and returns [`Exit::Failure`] when the session
    /// doesn't end as LSP says it should, such as with `exit` before
    /// `shutdown`, or the transport fails.
    pub(crate) fn run(self) -> Result<(), Exit> {
        // The server starts its own workers, and a large file's lint runs on
        // rayon's pool.
        let _already_built = ThreadPoolBuilder::new().stack_size(MIN_STACK_BYTES).build_global();
        let served = self.socket.map_or_else(luck_lsp::serve_stdio, luck_lsp::serve_socket);
        served.map_err(|e| {
            eprintln!("luck lsp: {e}");
            Exit::Failure
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
