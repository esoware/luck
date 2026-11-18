use std::io::{self, Write as _};

use tokio::io::{AsyncRead, AsyncWrite, split, stdin, stdout};
use tokio::net::TcpListener;
use tower_lsp_server::{Client, LspService, Server};

use crate::backend::Backend;

pub async fn serve_stdio() {
    serve(stdin(), stdout()).await;
}

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

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use tokio::io::{
        AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _,
        BufReader, duplex, split,
    };
    use tokio::join;
    use tokio::time::timeout;

    use super::serve;

    async fn send(writer: &mut (impl AsyncWrite + Unpin), body: &str) {
        let message = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        writer.write_all(message.as_bytes()).await.expect("the server reads");
    }

    /// Skips the server's messages up to the first that contains `needle`.
    async fn read_past(reader: &mut BufReader<impl AsyncRead + Unpin>, needle: &str) {
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).await.expect("the server writes");
            let length = header.trim_end().strip_prefix("Content-Length: ").expect("a header");
            reader.read_line(&mut String::new()).await.expect("a blank line");
            let mut body = vec![0; length.parse().expect("a length")];
            reader.read_exact(&mut body).await.expect("the body");
            if String::from_utf8(body).expect("UTF-8").contains(needle) {
                return;
            }
        }
    }

    #[tokio::test]
    async fn exit_ends_the_server_while_it_waits_on_the_client() {
        let (client, server) = duplex(1 << 16);
        let (server_reader, server_writer) = split(server);
        let (from_server, mut client_writer) = split(client);
        let mut client_reader = BufReader::new(from_server);
        let session = async {
            let initialize =
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
            send(&mut client_writer, initialize).await;
            read_past(&mut client_reader, r#""id":1"#).await;
            send(&mut client_writer, r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#)
                .await;
            // Left unanswered, as a client may leave it.
            read_past(&mut client_reader, "client/registerCapability").await;
            send(&mut client_writer, r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#).await;
            read_past(&mut client_reader, r#""result":null,"id":2"#).await;
            send(&mut client_writer, r#"{"jsonrpc":"2.0","method":"exit"}"#).await;
        };
        let ended = timeout(Duration::from_secs(30), async {
            join!(serve(server_reader, server_writer), session)
        });
        assert!(ended.await.is_ok(), "the server still serves after `exit`");
    }
}
