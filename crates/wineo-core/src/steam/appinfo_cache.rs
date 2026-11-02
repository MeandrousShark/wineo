//! The parsed `appinfo.vdf` kept between refreshes, for a caller that rebuilds
//! its library every few seconds. Following `stats::Cache`: the file is read
//! again only when its stamp changes.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use super::Install;
use super::appinfo::{self, AppInfo, ReadError};
use crate::files::Stamp;

/// A file that can't be read is kept too, so a broken one isn't parsed again
/// every tick. Behind a lock so a refresh can borrow it shared, like the rest
/// of its snapshot.
#[derive(Default)]
pub struct AppInfoCache {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    cached: Option<Cached>,
    /// How many times the file was read and parsed, for tests that pin a hit.
    #[cfg(any(test, feature = "test-support"))]
    reads: usize,
}

struct Cached {
    path: PathBuf,
    stamp: Stamp,
    result: Arc<Result<AppInfo, ReadError>>,
}

impl AppInfoCache {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn reads(&self) -> usize {
        self.state().reads
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The install's appinfo, read only if its file changed since the cached
    /// read. The result is kept under the stamp from before the read, so a
    /// write during the read counts as a change and the next call reads again.
    pub fn read(&self, install: &Install) -> Arc<Result<AppInfo, ReadError>> {
        self.read_using(install, appinfo::read)
    }

    fn read_using(
        &self,
        install: &Install,
        read: impl FnOnce(&Install) -> Result<AppInfo, ReadError>,
    ) -> Arc<Result<AppInfo, ReadError>> {
        let path = appinfo::path(install);
        let mut state = self.state();
        let before = Stamp::of(&path).ok().flatten();
        if let (Some(before), Some(cached)) = (before, &state.cached)
            && cached.path == path
            && cached.stamp == before
        {
            return Arc::clone(&cached.result);
        }
        state.cached = None;
        #[cfg(any(test, feature = "test-support"))]
        {
            state.reads += 1;
        }
        let result = Arc::new(read(install));
        if let Some(stamp) = before {
            state.cached = Some(Cached {
                path,
                stamp,
                result: Arc::clone(&result),
            });
        }
        result
    }
}

#[cfg(test)]
mod tests;
