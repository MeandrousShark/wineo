//! Shared with build.rs, which can't use the crate it builds.

/// The version to report: `crate_version`, plus SemVer build metadata naming
/// the commit unless the build is exactly the release.
///
/// `describe` is the output of `git describe --tags --match 'v[0-9]*'
/// --dirty --long`, or None without git or without a reachable tag. A tag
/// that isn't `v{crate_version}` still gets metadata, even at distance 0, so
/// only the tagged release commit can print the bare version.
pub fn version(crate_version: &str, describe: Option<&str>) -> String {
    match describe.and_then(|describe| metadata(crate_version, describe)) {
        Some(metadata) => format!("{crate_version}+{metadata}"),
        None => crate_version.to_owned(),
    }
}

fn metadata(crate_version: &str, describe: &str) -> Option<String> {
    let describe = describe.trim();
    let (rest, dirty) = match describe.strip_suffix("-dirty") {
        Some(rest) => (rest, true),
        None => (describe, false),
    };
    // Tags can hold dashes, so the count and hash are taken from the end.
    let mut parts = rest.rsplitn(3, '-');
    let hash = parts.next()?.strip_prefix('g')?;
    let count = parts.next()?;
    let tag = parts.next()?;
    let all = |text: &str, ok: fn(&u8) -> bool| !text.is_empty() && text.bytes().all(|b| ok(&b));
    if !all(hash, u8::is_ascii_hexdigit) || !all(count, u8::is_ascii_digit) {
        return None;
    }
    if !dirty && count == "0" && tag.strip_prefix('v') == Some(crate_version) {
        return None;
    }
    let mut metadata = format!("{count}.g{hash}");
    if dirty {
        metadata.push_str(".dirty");
    }
    Some(metadata)
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn the_release_tag_prints_the_plain_version() {
        assert_eq!(version("1.4.1", Some("v1.4.1-0-g79df1ad8\n")), "1.4.1");
    }

    #[test]
    fn commits_after_the_tag_name_the_commit() {
        assert_eq!(
            version("1.4.1", Some("v1.4.1-37-g79df1ad8")),
            "1.4.1+37.g79df1ad8"
        );
    }

    #[test]
    fn uncommitted_changes_are_marked_dirty() {
        assert_eq!(
            version("1.4.1", Some("v1.4.1-37-g79df1ad8-dirty")),
            "1.4.1+37.g79df1ad8.dirty"
        );
        // A dirty tree at the tag isn't the release either.
        assert_eq!(
            version("1.4.1", Some("v1.4.1-0-g79df1ad8-dirty")),
            "1.4.1+0.g79df1ad8.dirty"
        );
    }

    #[test]
    fn no_git_or_no_reachable_tag_prints_the_plain_version() {
        assert_eq!(version("1.4.1", None), "1.4.1");
    }

    #[test]
    fn a_tag_for_another_version_still_names_the_commit() {
        // The version was bumped after v1.4.1 but isn't tagged yet.
        assert_eq!(
            version("1.5.0", Some("v1.4.1-3-g79df1ad8")),
            "1.5.0+3.g79df1ad8"
        );
        assert_eq!(
            version("1.5.0", Some("v1.4.1-0-g79df1ad8")),
            "1.5.0+0.g79df1ad8"
        );
        assert_eq!(
            version("1.5.0", Some("v1.5.0-rc1-0-g79df1ad8")),
            "1.5.0+0.g79df1ad8"
        );
    }

    #[test]
    fn output_that_isnt_describe_falls_back_to_the_plain_version() {
        for describe in [
            "",
            "v1.4.1",
            "v1.4.1-x-g79df1ad8",
            "v1.4.1-3-79df1ad8",
            "v1.4.1-3-gxyz",
        ] {
            assert_eq!(version("1.4.1", Some(describe)), "1.4.1", "{describe}");
        }
    }

    #[test]
    fn metadata_is_semver_build_metadata() {
        let stamped = version("1.4.1", Some("v1.4.1-37-g79df1ad8-dirty"));
        let (_, metadata) = stamped.split_once('+').unwrap();
        for identifier in metadata.split('.') {
            assert!(!identifier.is_empty());
            assert!(
                identifier
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            );
        }
    }
}
