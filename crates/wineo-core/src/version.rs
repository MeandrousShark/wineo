//! The version wineo reports. build.rs computes it once, so the CLI and the
//! desktop app can't disagree.

/// `X.Y.Z` for a release build or a source tree without git, otherwise
/// `X.Y.Z+N.gHASH`, with `.dirty` for uncommitted changes.
pub const VERSION: &str = env!("WINEO_BUILD_VERSION");

// build.rs includes the same file, which is where the real work happens.
#[cfg(test)]
mod stamp;

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn the_version_starts_with_the_crate_version() {
        let crate_version = env!("CARGO_PKG_VERSION");
        assert!(
            VERSION == crate_version || VERSION.starts_with(&format!("{crate_version}+")),
            "{VERSION}"
        );
    }
}
