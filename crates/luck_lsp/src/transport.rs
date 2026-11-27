use std::io;
use std::num::NonZero;

use lsp_server::Connection;

use crate::server::serve;

/// Serves one client over stdin and stdout, on `threads` threads.
///
/// # Errors
///
/// As [`serve`] does, or with the error from reading stdin or writing
/// stdout.
pub fn serve_stdio(threads: NonZero<usize>) -> io::Result<()> {
    let (connection, io_threads) = Connection::stdio();
    let served = serve(connection, threads);
    let joined = io_threads.join();
    served.and(joined)
}
