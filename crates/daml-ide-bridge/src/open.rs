//! Opening a URL in the user's browser.

use std::io;
use std::process::{Command, Stdio};

/// Waits for the launcher to exit, which is immediate, so the caller can report
/// whether it worked. A browser that silently fails to appear is the hardest
/// kind of failure to diagnose from the editor.
pub fn url(url: &str) -> io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/c", "start", "", url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?
        .wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the browser launcher exited with {status}"
        )))
    }
}
