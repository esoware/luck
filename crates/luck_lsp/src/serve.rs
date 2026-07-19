//! LSP transport entry points, callable from the `luck` CLI.

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tower_lsp::Server;

use crate::backend::build_service;

pub async fn serve_stdio() {
    serve(tokio::io::stdin(), tokio::io::stdout()).await;
}

/// Bind 127.0.0.1:<port>, accept one client, serve LSP over it.
pub async fn serve_socket(port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    eprintln!("luck lsp: listening on 127.0.0.1:{port}");
    let (stream, addr) = listener.accept().await?;
    eprintln!("luck lsp: accepted connection from {addr}");
    let (reader, writer) = tokio::io::split(stream);
    serve(reader, writer).await;
    Ok(())
}

async fn serve<R, W>(reader: R, writer: W)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (service, socket) = build_service();
    Server::new(reader, writer, socket).serve(service).await;
}
