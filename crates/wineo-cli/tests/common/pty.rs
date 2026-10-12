//! wineo on the far side of a pseudo-terminal, for commands that need a
//! terminal to read from, like the GOG login.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::fs::OFlags;
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

pub struct Pty {
    master: File,
    child: Child,
    seen: Arc<Mutex<String>>,
    /// How much of the output earlier expectations have used up.
    cursor: usize,
    reader: Option<JoinHandle<()>>,
}

impl Pty {
    pub fn spawn(mut command: Command) -> Self {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let name = ptsname(&master, Vec::new()).unwrap();
        let slave = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(OFlags::NOCTTY.bits() as i32)
            .open(name.to_str().unwrap())
            .unwrap();
        // Its own process group, so the whole group can be killed if the
        // test gives up on it.
        command
            .process_group(0)
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave);
        let child = command.spawn().unwrap();
        // The command holds the only other copies of the terminal's end, and
        // dropping them is what lets reading see the end once wineo exits.
        drop(command);

        let master = File::from(master);
        let mut output = master.try_clone().unwrap();
        let seen = Arc::new(Mutex::new(String::new()));
        let collected = seen.clone();
        let reader = thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = output.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&buffer[..read]).replace('\r', "");
                collected.lock().unwrap().push_str(&text);
            }
        });
        Self {
            master,
            child,
            seen,
            cursor: 0,
            reader: Some(reader),
        }
    }

    /// Waits for `text` to show up after whatever was expected last.
    #[track_caller]
    pub fn expect(&mut self, text: &str) {
        self.expect_within(text, Duration::from_secs(20));
    }

    #[track_caller]
    pub fn expect_within(&mut self, text: &str, limit: Duration) {
        let deadline = Instant::now() + limit;
        loop {
            {
                let seen = strip_escapes(&self.seen.lock().unwrap());
                if let Some(at) = seen[self.cursor..].find(text) {
                    self.cursor += at + text.len();
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {text:?} in:\n{}",
                    redact(&seen[self.cursor..])
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Types `line` and presses Enter, which a terminal sends as `\r`.
    pub fn send(&mut self, line: &str) {
        self.master
            .write_all(format!("{line}\r").as_bytes())
            .unwrap();
    }

    /// Ctrl+C typed at a question, which the line editor reads as a key.
    pub fn ctrl_c(&mut self) {
        self.master.write_all(b"\x03").unwrap();
    }

    /// Waits up to 20 seconds for wineo to exit. The status, and everything
    /// it printed.
    pub fn finish(self) -> (ExitStatus, Seen) {
        self.finish_within(Duration::from_secs(20))
    }

    /// A command still running when time runs out fails the test instead of
    /// hanging it, and is killed.
    pub fn finish_within(mut self, limit: Duration) -> (ExitStatus, Seen) {
        let deadline = Instant::now() + limit;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                self.kill();
                let seen = strip_escapes(&self.seen.lock().unwrap());
                panic!("wineo didn't exit; it had printed:\n{}", redact(&seen));
            }
            thread::sleep(Duration::from_millis(20));
        };
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let seen = strip_escapes(&self.seen.lock().unwrap());
        (status, Seen(seen))
    }

    /// The process id of the command.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Kills the command's whole process group and reaps it.
    fn kill(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        let group = format!("-{}", self.child.id());
        let _ = Command::new("kill").args(["-KILL", "--", &group]).status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Pty {
    /// A test that panics, or drops the terminal early, leaves nothing
    /// running behind it.
    fn drop(&mut self) {
        self.kill();
    }
}

/// What a command printed to its terminal. It may hold secrets a test is
/// checking for, so printing it, in an assertion message or anywhere else,
/// hides them; `raw` is for the check itself.
pub struct Seen(String);

impl Seen {
    pub fn contains(&self, text: &str) -> bool {
        self.0.contains(text)
    }

    pub fn raw(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Seen {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&redact(&self.0))
    }
}

impl fmt::Debug for Seen {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}", redact(&self.0))
    }
}

/// Hides every word that looks like one of the fake GOG's secrets, or like
/// what the tests paste.
pub fn redact(text: &str) -> String {
    text.split_inclusive(char::is_whitespace)
        .map(|word| {
            if ["SECRET", "GoodCode"]
                .iter()
                .any(|secret| word.contains(secret))
            {
                let tail = &word[word.trim_end().len()..];
                format!("<redacted>{tail}")
            } else {
                word.to_owned()
            }
        })
        .collect()
}

/// The output without the escape sequences a line editor redraws with.
pub fn strip_escapes(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        if char != '\x1b' {
            plain.push(char);
            continue;
        }
        if chars.next_if_eq(&'[').is_some() {
            // Parameters, then one final byte from `@` to `~`.
            for char in chars.by_ref() {
                if ('@'..='~').contains(&char) {
                    break;
                }
            }
        } else {
            chars.next();
        }
    }
    plain
}
