//! Test-only helpers that write executable files. Shared by every test target
//! through `#[path]`, so there is one copy.

#![allow(dead_code)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes an executable script with mode 0o755; see [`write_executable`].
pub fn write_script(path: impl AsRef<Path>, body: impl AsRef<[u8]>) {
    write_executable(path, body, 0o755);
}

/// Writes `body` to `path` and sets `mode`, using a child process for both.
///
/// Tests run on parallel threads, and any thread that forks hands every open
/// descriptor to its child until that child execs. If this process held the
/// file open for writing at that moment, running the file would fail with
/// ETXTBSY ("Text file busy"). Writing from a child keeps the writable
/// descriptor out of the test process, so use this instead of `fs::write`
/// followed by `set_permissions` for anything a test goes on to execute.
pub fn write_executable(path: impl AsRef<Path>, body: impl AsRef<[u8]>, mode: u32) {
    let path = path.as_ref();
    let mut child = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod \"$2\" \"$1\"", "sh"])
        .arg(path)
        .arg(format!("{mode:o}"))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_ref())
        .unwrap();
    assert!(
        child.wait().unwrap().success(),
        "writing {}",
        path.display()
    );
}
