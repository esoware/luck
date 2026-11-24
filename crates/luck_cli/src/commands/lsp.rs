use std::num::NonZero;

use clap::Args;

use crate::exit::Exit;

#[derive(Args)]
pub(crate) struct LspArgs {
    /// Listen on 127.0.0.1 at this port and serve one client instead of stdio.
    #[arg(long)]
    socket: Option<u16>,
}

impl LspArgs {
    pub(crate) fn run(self, threads: NonZero<usize>) -> Result<(), Exit> {
        let served = match self.socket {
            Some(port) => luck_lsp::serve_socket(port, threads),
            None => luck_lsp::serve_stdio(threads),
        };
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
    use crate::cli::Cli;
    use crate::commands::Command;

    #[test]
    fn lsp_subcommand_parses() {
        let cli = Cli::try_parse_from(["luck", "lsp"]).expect("lsp parses");
        assert!(matches!(cli.command, Command::Lsp(LspArgs { socket: None })));
        let with_socket = Cli::try_parse_from(["luck", "lsp", "--socket", "9000"]).unwrap();
        assert!(matches!(with_socket.command, Command::Lsp(LspArgs { socket: Some(9000) })));
    }
}
