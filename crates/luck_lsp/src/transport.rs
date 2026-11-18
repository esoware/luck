use std::io::{self, Write as _};

use lsp_server::Connection;

use crate::server::serve;

/// Serves one client over stdin and stdout.
///
/// # Errors
///
/// As [`serve`] does, or with the error from reading stdin or writing
/// stdout.
pub fn serve_stdio() -> io::Result<()> {
    let (connection, io_threads) = Connection::stdio();
    let served = serve(connection);
    let joined = io_threads.join();
    served.and(joined)
}

/// Serves the first client to connect on 127.0.0.1 at `port`, for
/// debugging.
///
/// # Errors
///
/// As [`serve`] does, or with the error from binding the port, accepting the
/// client, reading or writing the socket, or writing a status line to stderr.
pub fn serve_socket(port: u16) -> io::Result<()> {
    writeln!(io::stderr(), "luck lsp: waiting for a client on 127.0.0.1:{port}")?;
    let (connection, io_threads) = Connection::listen(("127.0.0.1", port))?;
    writeln!(io::stderr(), "luck lsp: a client connected")?;
    let served = serve(connection);
    let joined = io_threads.join();
    served.and(joined)
}
