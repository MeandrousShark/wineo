use std::io::{self, Write};

// Match default SIGPIPE termination so output cut short never reports success.
pub const CLOSED_PIPE: u8 = 141;

pub fn handle(result: io::Result<()>) {
    if let Err(error) = handle_pipe(result) {
        panic!("failed printing output: {error}");
    }
}

fn handle_pipe<T>(result: io::Result<T>) -> io::Result<T> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
            std::process::exit(i32::from(CLOSED_PIPE));
        }
        result => result,
    }
}

// Handle own output at the writer so helpers that swallow IO errors cannot
// turn a closed pipe into success. Errors from other writers stay ordinary errors.
pub struct Stream<W>(W);

pub fn stdout() -> Stream<io::Stdout> {
    Stream(io::stdout())
}

pub fn stderr() -> Stream<io::Stderr> {
    Stream(io::stderr())
}

impl Stream<io::Stdout> {
    pub fn lock(&self) -> Stream<io::StdoutLock<'_>> {
        Stream(self.0.lock())
    }
}

impl<W: Write> Write for Stream<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        handle_pipe(self.0.write(bytes))
    }

    fn flush(&mut self) -> io::Result<()> {
        handle_pipe(self.0.flush())
    }
}

pub fn is_clap_broken_pipe(error: &clap::Error) -> bool {
    if error.kind() != clap::error::ErrorKind::Io {
        return false;
    }
    // clap stores IO errors as text, without their original source.
    let pipe = io::Error::from_raw_os_error(rustix::io::Errno::PIPE.raw_os_error()).to_string();
    error.to_string().contains(&pipe)
}
