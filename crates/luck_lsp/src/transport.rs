//! LSP transport entry points, callable from the `luck` CLI.

use std::io::{self, Write as _};

use tokio::io::{AsyncRead, AsyncWrite, split, stdin, stdout};
use tokio::net::TcpListener;
use tower_lsp::{Client, LspService, Server};

use crate::backend::Backend;

/// Serves LSP over stdin and stdout until the client exits.
pub async fn serve_stdio() {
    serve(stdin(), stdout()).await;
}

/// Binds `127.0.0.1` on `port`, accepts one client, and serves LSP to it.
///
/// # Errors
///
/// Returns the I/O error from binding the port, accepting the client, or
/// writing a status line to stderr.
pub async fn serve_socket(port: u16) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    writeln!(io::stderr(), "luck lsp: listening on 127.0.0.1:{port}")?;
    let (stream, address) = listener.accept().await?;
    writeln!(io::stderr(), "luck lsp: accepted connection from {address}")?;
    let (reader, writer) = split(stream);
    serve(reader, writer).await;
    Ok(())
}

/// Serves the backend over the real `tower_lsp::Client`, with the
/// `luck/syntaxTree` and `luck/fixAllWorkspace` custom requests.
async fn serve<R, W>(reader: R, writer: W)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (service, socket) = LspService::build(Backend::new)
        .custom_method("luck/syntaxTree", Backend::<Client>::syntax_tree_request)
        .custom_method("luck/fixAllWorkspace", Backend::<Client>::fix_all_workspace_request)
        .finish();
    Server::new(reader, writer, socket).serve(service).await;
}
