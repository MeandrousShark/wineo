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
    // `--` keeps a file name starting with `-` from reading as an option.
    let mut child = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod \"$2\" -- \"$1\"", "sh"])
        .arg(path)
        .arg(format!("{mode:o}"))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
    // Close stdin and reap the child before reporting a failed write, so a
    // failure never leaves a zombie behind.
    let written = {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(body.as_ref())
    };
    let status = child.wait();
    match (written, status) {
        (Ok(()), Ok(status)) if status.success() => {}
        (written, status) => panic!(
            "writing {}: write {written:?}, exit {status:?}",
            path.display()
        ),
    }
}
