use std::io::{self, Write};

use anyhow::{Context, Result};

/// Writes one line to stdout. Returns false when the reader closed the
/// pipe (e.g. `scwx ls | head`), which ends output but is not an error;
/// `println!` would panic instead, and the release profile aborts on panic.
pub(crate) fn emit(text: &str) -> Result<bool> {
    let mut stdout = io::stdout().lock();
    match writeln!(stdout, "{text}") {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(error) => Err(error).context("writing to stdout"),
    }
}
