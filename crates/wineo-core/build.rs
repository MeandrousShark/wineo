//! Stamps the version wineo reports with the commit it was built from. A
//! source tree without git, like the vendored tarball the distribution
//! packages build from, gets the plain crate version, and so does any build
//! where git fails: the stamp must never fail the build.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[path = "src/version/stamp.rs"]
mod stamp;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/version/stamp.rs");
    let crate_version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let describe = env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .and_then(|manifest| describe(&manifest.join("../..")));
    println!(
        "cargo:rustc-env=WINEO_BUILD_VERSION={}",
        stamp::version(&crate_version, describe.as_deref())
    );
}

fn describe(workspace: &Path) -> Option<String> {
    let workspace = workspace.canonicalize().ok()?;
    let found = git(
        &workspace,
        &[
            "rev-parse",
            "--show-toplevel",
            "--git-path",
            "HEAD",
            "--git-common-dir",
            "--symbolic-full-name",
            "HEAD",
        ],
    )?;
    let lines: Vec<&str> = found.lines().collect();
    let [top, head, common, head_ref] = lines[..] else {
        return None;
    };
    // An unpacked tarball inside some other checkout, like an AUR or Debian
    // packaging repository, must not be stamped with that repository's
    // commit.
    if Path::new(top).canonicalize().ok()? != workspace {
        return None;
    }
    // rev-parse prints these relative to the directory it ran in.
    let common = workspace.join(common);
    // The index isn't watched: agents stage constantly, and every rerun
    // rebuilds core, the CLI, and the GUI. So `.dirty` reflects the tree when
    // the stamp was last computed, which is fine because acceptance builds
    // come from clean committed branches.
    let mut watched = vec![workspace.join(head)];
    if head_ref.starts_with("refs/") {
        // A packed ref has no file yet, and the next commit creates one in
        // its directory.
        let mut path = common.join(head_ref);
        while !path.exists() && path.pop() && path.starts_with(&common) {}
        watched.push(path);
    }
    // A new tag at HEAD turns a stamped build into the release. A reftable
    // repository keeps every ref in its tables instead.
    watched
        .extend(["packed-refs", "refs/tags", "reftable/tables.list"].map(|path| common.join(path)));
    for path in watched {
        // Cargo reruns on every build for a path that doesn't exist.
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    let describe = git(
        &workspace,
        &[
            "describe",
            "--tags",
            "--match",
            "v[0-9]*",
            "--long",
            "--abbrev=8",
        ],
    )?;
    // Not `describe --dirty`, which takes index.lock and rewrites a stale
    // index even under --no-optional-locks, so it could fail a `git add` or
    // commit running beside the build. status refreshes only in memory. If
    // it fails, the build is marked dirty rather than passed off as the
    // release.
    let dirty = git(
        &workspace,
        &["status", "--porcelain", "--untracked-files=no"],
    )
    .is_none_or(|status| !status.trim().is_empty());
    let describe = describe.trim();
    Some(if dirty {
        format!("{describe}-dirty")
    } else {
        describe.to_owned()
    })
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        // Builds run beside the developer's own git commands, so status must
        // not write the refreshed index back.
        .arg("--no-optional-locks")
        .args(args);
    // Find the repository from the source tree, not from a caller's hook or
    // script. This also drops config passed through GIT_CONFIG_*, so a
    // safe.directory set that way leaves git unreadable, and the build prints
    // the plain version as it does with no git at all.
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}
