//! Test-only: which files the code under test read on this thread, so a test
//! can pin that a batch reads each one once.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    static READS: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Records that `path` is about to be read.
pub(crate) fn note(path: &Path) {
    READS.with(|reads| reads.borrow_mut().push(path.to_owned()));
}

/// Forgets what was read so far.
pub(crate) fn clear() {
    READS.with(|reads| reads.borrow_mut().clear());
}

/// How many times a file with this name was read since the last `clear`.
pub(crate) fn count(file_name: &str) -> usize {
    READS.with(|reads| {
        reads
            .borrow()
            .iter()
            .filter(|path| path.file_name().is_some_and(|name| name == file_name))
            .count()
    })
}
