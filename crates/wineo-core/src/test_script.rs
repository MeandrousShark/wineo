use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes an executable script from a child process. When this process holds
/// the file open for writing, any test thread that forks at that moment hands
/// the descriptor to its child until that child execs, and running the script
/// meanwhile fails with "Text file busy".
pub(crate) fn write_script(path: &Path, body: &str) {
    let mut child = Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    assert!(
        child.wait().unwrap().success(),
        "writing {}",
        path.display()
    );
}
