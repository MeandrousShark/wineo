use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use super::*;
use crate::download::Response;
use crate::epic::fake::Setup;

const URL: &str = "https://fixture/legendary_linux_x64";

/// A server with one file, or none, that counts the requests it gets and
/// can hand the body over slowly.
struct Serve {
    status: u16,
    body: Vec<u8>,
    hits: AtomicUsize,
    slow: bool,
}

impl Serve {
    fn file(body: &[u8]) -> Self {
        Self {
            status: 200,
            body: body.to_vec(),
            hits: AtomicUsize::new(0),
            slow: false,
        }
    }
}

impl Transport for Serve {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn Read>> {
        anyhow::bail!("only request is used, not get: {url}")
    }

    fn request(&self, url: &str, bearer: Option<&str>) -> anyhow::Result<Response> {
        assert_eq!(url, URL);
        assert_eq!(bearer, None);
        self.hits.fetch_add(1, Ordering::SeqCst);
        let body: Box<dyn Read> = if self.slow {
            Box::new(Slow(Cursor::new(self.body.clone())))
        } else {
            Box::new(Cursor::new(self.body.clone()))
        };
        Ok(Response {
            status: self.status,
            body,
        })
    }
}

/// Hands over a few bytes at a time, with a pause before each.
struct Slow(Cursor<Vec<u8>>);

impl Read for Slow {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        thread::sleep(Duration::from_millis(20));
        let end = buffer.len().min(4);
        self.0.read(&mut buffer[..end])
    }
}

const BODY: &[u8] = b"#!/usr/bin/env python3\nnot really a zipapp\n";

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn pin(sha256: &str) -> Pin<'_> {
    Pin {
        version: "9.9.9",
        url: URL,
        size: BODY.len() as u64,
        sha256,
    }
}

fn leftovers(setup: &Setup) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(crate::epic::bin_dir(&setup.paths))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".part-"))
        .collect();
    names.sort();
    names
}

#[test]
fn a_matching_download_is_put_in_place_privately() {
    let setup = Setup::new();
    let serve = Serve::file(BODY);
    let hash = sha(BODY);
    let mut seen = Vec::new();
    let fetched = download(
        &setup.paths,
        &serve,
        &pin(&hash),
        &Token::new(),
        |got, of| {
            seen.push((got, of));
        },
    )
    .unwrap();
    let path = file(&setup.paths, "9.9.9");
    assert_eq!(fetched, Fetched::Downloaded(path.clone()));
    assert_eq!(fs::read(&path).unwrap(), BODY);
    assert_eq!(
        path,
        setup.paths.host_data().join("epic/bin/legendary-9.9.9")
    );
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o700);
    assert_eq!(mode(&crate::epic::bin_dir(&setup.paths)), 0o700);
    assert_eq!(seen.last(), Some(&(BODY.len() as u64, BODY.len() as u64)));
    assert!(leftovers(&setup).is_empty());

    let again = download(&setup.paths, &serve, &pin(&hash), &Token::new(), |_, _| {}).unwrap();
    assert_eq!(again, Fetched::AlreadyThere(path));
    assert_eq!(serve.hits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_download_with_the_wrong_hash_is_refused() {
    let setup = Setup::new();
    let wrong = sha(b"something else");
    let error = download(
        &setup.paths,
        &Serve::file(BODY),
        &pin(&wrong),
        &Token::new(),
        |_, _| {},
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Couldn't get Legendary 9.9.9: the download doesn't match what this wineo expects.\n\
         Install Legendary yourself and set [epic] legendary, or update wineo."
    );
    assert!(!file(&setup.paths, "9.9.9").exists());
    assert!(leftovers(&setup).is_empty());
}

#[test]
fn a_download_cut_short_is_refused() {
    let setup = Setup::new();
    let hash = sha(BODY);
    let short = Serve::file(&BODY[..10]);
    let error = download(&setup.paths, &short, &pin(&hash), &Token::new(), |_, _| {}).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "Couldn't get Legendary 9.9.9: the download ended after 10 of {} bytes.",
            BODY.len()
        )
    );
    assert!(!file(&setup.paths, "9.9.9").exists());
    assert!(leftovers(&setup).is_empty());
}

#[test]
fn a_download_longer_than_pinned_stops_at_the_size() {
    let setup = Setup::new();
    let hash = sha(BODY);
    let mut long = BODY.to_vec();
    long.extend_from_slice(&[b'x'; 1024 * 1024]);
    let serve = Serve::file(&long);
    let mut most = 0;
    let error = download(
        &setup.paths,
        &serve,
        &pin(&hash),
        &Token::new(),
        |got, _| {
            most = most.max(got);
        },
    )
    .unwrap_err();
    assert!(matches!(error, Error::Mismatch { .. }), "{error}");
    assert!(most <= BODY.len() as u64, "kept reading to {most}");
    assert!(!file(&setup.paths, "9.9.9").exists());
    assert!(leftovers(&setup).is_empty());
}

#[test]
fn a_download_that_isnt_there_says_so() {
    let setup = Setup::new();
    let hash = sha(BODY);
    for status in [404, 410] {
        let gone = Serve {
            status,
            ..Serve::file(b"")
        };
        let error =
            download(&setup.paths, &gone, &pin(&hash), &Token::new(), |_, _| {}).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Couldn't get Legendary 9.9.9: the download is no longer there.\n\
             Install Legendary yourself and set [epic] legendary, or update wineo."
        );
    }
    let broken = Serve {
        status: 500,
        ..Serve::file(b"")
    };
    let error = download(&setup.paths, &broken, &pin(&hash), &Token::new(), |_, _| {}).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Couldn't get Legendary 9.9.9: the server answered 500"
    );
    assert!(!file(&setup.paths, "9.9.9").exists());
}

#[test]
fn two_downloads_at_once_fetch_it_once() {
    let setup = Arc::new(Setup::new());
    let serve = Arc::new(Serve {
        slow: true,
        ..Serve::file(BODY)
    });
    let hash = Arc::new(sha(BODY));
    let both: Vec<_> = (0..2)
        .map(|_| {
            let (setup, serve, hash) = (setup.clone(), serve.clone(), hash.clone());
            thread::spawn(move || {
                download(&setup.paths, &*serve, &pin(&hash), &Token::new(), |_, _| {}).unwrap()
            })
        })
        .collect();
    let mut results: Vec<_> = both
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    results.sort_by_key(|fetched| matches!(fetched, Fetched::AlreadyThere(_)));
    let path = file(&setup.paths, "9.9.9");
    assert_eq!(
        results,
        [
            Fetched::Downloaded(path.clone()),
            Fetched::AlreadyThere(path.clone())
        ]
    );
    assert_eq!(serve.hits.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read(&path).unwrap(), BODY);
}

#[test]
fn a_leftover_from_a_run_that_died_is_removed_under_the_lock() {
    let setup = Setup::new();
    let dir = crate::epic::bin_dir(&setup.paths);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(".legendary-9.9.9.part-abc123"), b"half").unwrap();
    // Another version's leftover belongs to that version's download.
    fs::write(dir.join(".legendary-1.0.0.part-abc123"), b"other").unwrap();
    let hash = sha(BODY);
    download(
        &setup.paths,
        &Serve::file(BODY),
        &pin(&hash),
        &Token::new(),
        |_, _| {},
    )
    .unwrap();
    assert_eq!(leftovers(&setup), [".legendary-1.0.0.part-abc123"]);
}

#[test]
fn a_canceled_download_leaves_nothing() {
    let setup = Setup::new();
    let hash = sha(BODY);
    let token = Token::new();
    token.cancel();
    let error = download(
        &setup.paths,
        &Serve::file(BODY),
        &pin(&hash),
        &token,
        |_, _| {},
    )
    .unwrap_err();
    assert!(error.is_canceled(), "{error}");
    assert!(!file(&setup.paths, "9.9.9").exists());
    assert!(leftovers(&setup).is_empty());
}

#[test]
fn older_copies_go_only_when_nothing_runs_from_them() {
    let setup = Setup::new();
    let dir = crate::epic::bin_dir(&setup.paths);
    fs::create_dir_all(&dir).unwrap();
    for name in ["legendary-0.21.0", "legendary-0.20.0", "legendary-9.9.9"] {
        fs::write(dir.join(name), name).unwrap();
    }
    let busy = File::open(dir.join("legendary-0.20.0")).unwrap();
    flock(&busy, FlockOperation::LockShared).unwrap();

    let removed = remove_older(&setup.paths, "9.9.9").unwrap();
    assert_eq!(removed, [dir.join("legendary-0.21.0")]);
    assert!(dir.join("legendary-0.20.0").exists());
    assert!(dir.join("legendary-9.9.9").exists());

    drop(busy);
    assert_eq!(
        remove_older(&setup.paths, "9.9.9").unwrap(),
        [dir.join("legendary-0.20.0")]
    );
}

#[test]
fn older_copies_wait_for_the_epic_lock() {
    let setup = Arc::new(Setup::new());
    let dir = crate::epic::bin_dir(&setup.paths);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("legendary-0.21.0"), "old").unwrap();
    let lock = EpicLock::acquire(&setup.paths).unwrap();
    let remover = {
        let setup = setup.clone();
        thread::spawn(move || remove_older(&setup.paths, "9.9.9").unwrap())
    };
    thread::sleep(Duration::from_millis(200));
    assert!(dir.join("legendary-0.21.0").exists());
    drop(lock);
    assert_eq!(remover.join().unwrap(), [dir.join("legendary-0.21.0")]);
}

#[test]
fn the_pin_is_the_release_file_the_probe_ran() {
    assert_eq!(PINNED.version, "0.21.1");
    assert_eq!(
        PINNED.url,
        "https://github.com/legendary-gl/legendary/releases/download/0.21.1/legendary_linux_x64"
    );
    assert_eq!(PINNED.size, 3_093_622);
    assert_eq!(
        PINNED.sha256,
        "dbed33bbe96031e65858233e4badf0a1d401bb6cb0cc995d1237cf3a0c826a45"
    );
}

#[test]
fn a_symlink_at_the_destination_is_an_error_and_left_alone() {
    let setup = Setup::new();
    let real = setup.root.path().join("elsewhere");
    fs::write(&real, BODY).unwrap();
    let path = file(&setup.paths, "9.9.9");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();
    let serve = Serve::file(BODY);
    let hash = sha(BODY);
    let error = download(&setup.paths, &serve, &pin(&hash), &Token::new(), |_, _| {}).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "Couldn't get Legendary 9.9.9: {} is in the way and isn't a regular file",
            path.display()
        )
    );
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&real).unwrap(), BODY);
    assert_eq!(serve.hits.load(Ordering::SeqCst), 0);
}

#[test]
fn the_question_names_the_pinned_version_and_its_size_in_whole_megabytes() {
    assert_eq!(
        question(),
        "Epic needs Legendary 0.21.1 (3 MB, from its GitHub releases). Download it?"
    );
}
