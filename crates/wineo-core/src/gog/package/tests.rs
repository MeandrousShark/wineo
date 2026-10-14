use std::fs;
use std::io::{Cursor, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::*;
use crate::cancel;
use crate::gog::folder::WinningEntry;

type Writer = ZipWriter<Cursor<Vec<u8>>>;

const SCRIPT: &[u8] = b"#!/bin/bash\n# a stand-in for GOG's split file script\n";

fn known() -> [u8; 32] {
    Sha256::digest(SCRIPT).into()
}

fn options(mode: u32) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(mode)
}

fn file(zip: &mut Writer, name: &str, content: &[u8], mode: u32) {
    zip.start_file(name, options(mode)).unwrap();
    zip.write_all(content).unwrap();
}

/// A file with mode bits `unix_permissions` would strip, like setuid or a
/// file type.
fn raw_mode_file(zip: &mut Writer, name: &str, content: &[u8], mode: u32) {
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .external_attributes(mode << 16);
    zip.start_file(name, options).unwrap();
    zip.write_all(content).unwrap();
}

fn dir(zip: &mut Writer, name: &str) {
    zip.add_directory(name, options(0o755)).unwrap();
}

fn link(zip: &mut Writer, name: &str, target: &str) {
    zip.add_symlink(name, target, options(0o777)).unwrap();
}

/// A `.sh` built like GOG's: a shell header, bytes standing in for
/// MojoSetup, then the zip.
fn installer(build: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    build(&mut zip);
    let zipped = zip.finish().unwrap().into_inner();
    let mut bytes =
        b"#!/bin/sh\n# This script was generated using Makeself 2.4.0\nexit 0\n".to_vec();
    bytes.extend(b"MOJOSETUP".repeat(500));
    bytes.extend(zipped);
    bytes
}

/// Replaces every occurrence of `from` with `to`, which has the same length.
fn patch(bytes: &mut [u8], from: &[u8], to: &[u8]) {
    assert_eq!(from.len(), to.len());
    let mut found = false;
    let mut at = 0;
    while let Some(offset) = bytes[at..].windows(from.len()).position(|w| w == from) {
        let start = at + offset;
        bytes[start..start + from.len()].copy_from_slice(to);
        at = start + from.len();
        found = true;
    }
    assert!(found, "nothing to patch");
}

fn save(dir: &Path, bytes: &[u8]) -> PathBuf {
    let path = dir.join("game_1.0.sh");
    fs::write(&path, bytes).unwrap();
    path
}

fn open_bytes(dir: &Path, bytes: &[u8]) -> Result<Package> {
    open_with(&save(dir, bytes), &[known()])
}

fn refusal(bytes: Vec<u8>) -> String {
    let temp = tempfile::tempdir().unwrap();
    let error = open_bytes(temp.path(), &bytes).unwrap_err();
    match error.downcast_ref::<PackageError>() {
        Some(PackageError::Invalid(detail)) => detail.clone(),
        other => panic!("expected an invalid package, got {other:?}: {error:#}"),
    }
}

fn md5_hex(bytes: &[u8]) -> String {
    crate::units::hex(Md5::digest(bytes))
}

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

struct Staged {
    _temp: tempfile::TempDir,
    root: PathBuf,
    entries: Vec<WinningEntry>,
    progress: Vec<StageProgress>,
}

fn stage_bytes(bytes: &[u8]) -> Staged {
    let temp = tempfile::tempdir().unwrap();
    let mut package = open_bytes(temp.path(), bytes).unwrap();
    let folder = temp.path().join("Games/game");
    fs::create_dir_all(&folder).unwrap();
    let mut progress = Vec::new();
    let entries = stage(
        &mut package,
        &folder,
        "op1",
        1207658691,
        &cancel::Token::new(),
        |event| progress.push(event),
    )
    .unwrap();
    Staged {
        root: folder.join(".wineo-op1-stage-1207658691"),
        _temp: temp,
        entries,
        progress,
    }
}

fn entry<'a>(entries: &'a [WinningEntry], path: &str) -> &'a WinningEntry {
    entries
        .iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("{path} wasn't staged: {entries:#?}"))
}

#[test]
fn a_plain_package_stages_its_game_files_only() {
    let data = b"level data ".repeat(10_000);
    let bytes = installer(|zip| {
        file(zip, "scripts/postinst", b"echo hi", 0o755);
        file(zip, "meta/info.txt", b"meta", 0o644);
        dir(zip, "data/noarch/");
        dir(zip, "data/noarch/docs/");
        file(zip, "data/noarch/game/start.sh", b"#!/bin/sh\n", 0o755);
        zip.start_file(
            "data/noarch/game/data.bin",
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .unix_permissions(0o644),
        )
        .unwrap();
        zip.write_all(&data).unwrap();
        link(zip, "data/noarch/game/current", "data.bin");
    });
    let staged = stage_bytes(&bytes);

    let paths: Vec<&str> = staged.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "docs",
            "game",
            "game/current",
            "game/data.bin",
            "game/start.sh"
        ]
    );
    for path in ["docs", "game"] {
        let staged_dir = entry(&staged.entries, path);
        assert_eq!(staged_dir.kind, EntryType::Directory);
        assert_eq!(staged_dir.mode, 0o755);
        assert!(staged.root.join(path).is_dir());
        assert_eq!(mode_of(&staged.root.join(path)), 0o755);
    }

    let data_entry = entry(&staged.entries, "game/data.bin");
    assert_eq!(data_entry.kind, EntryType::File);
    assert_eq!(data_entry.size, Some(data.len() as u64));
    assert_eq!(data_entry.md5.as_deref(), Some(md5_hex(&data).as_str()));
    assert_eq!(data_entry.mode, 0o644);
    assert_eq!(data_entry.product, 1207658691);
    assert_eq!(fs::read(staged.root.join("game/data.bin")).unwrap(), data);

    let start = entry(&staged.entries, "game/start.sh");
    assert_eq!(start.mode, 0o755);
    assert_eq!(start.md5.as_deref(), Some(md5_hex(b"#!/bin/sh\n").as_str()));
    assert_eq!(mode_of(&staged.root.join("game/start.sh")), 0o755);

    let current = entry(&staged.entries, "game/current");
    assert_eq!(current.kind, EntryType::Symlink);
    assert_eq!(current.target.as_deref(), Some("data.bin"));
    assert_eq!(current.size, None);
    assert_eq!(
        fs::read_link(staged.root.join("game/current")).unwrap(),
        Path::new("data.bin")
    );

    assert!(!staged.root.join("scripts").exists());
    assert!(!staged.root.join("meta").exists());
    assert!(!staged.root.join("data").exists());
    let last = staged.progress.last().unwrap();
    assert_eq!(last.done, last.total);
    assert_eq!(last.total, data.len() as u64 + 10);
}

#[test]
fn opening_reports_entries_and_sizes() {
    let bytes = installer(|zip| {
        file(zip, "data/noarch/a/b/c.txt", b"12345", 0o644);
        file(zip, "data/noarch/d.txt", b"123", 0o644);
    });
    let temp = tempfile::tempdir().unwrap();
    let package = open_bytes(temp.path(), &bytes).unwrap();
    let listed: Vec<(&str, EntryType)> = package
        .entries()
        .iter()
        .map(|entry| (entry.path.as_str(), entry.kind))
        .collect();
    assert_eq!(
        listed,
        [
            ("a", EntryType::Directory),
            ("a/b", EntryType::Directory),
            ("a/b/c.txt", EntryType::File),
            ("d.txt", EntryType::File),
        ]
    );
    assert_eq!(package.unpacked_size(), 8);
    assert_eq!(package.assembled_size(), 0);
}

#[test]
fn executable_bits_are_kept_and_unsafe_bits_dropped() {
    let bytes = installer(|zip| {
        raw_mode_file(zip, "data/noarch/setuid", b"x", 0o104_777);
        raw_mode_file(zip, "data/noarch/sticky", b"x", 0o101_744);
        file(zip, "data/noarch/writable", b"x", 0o666);
        file(zip, "data/noarch/owner_exec", b"x", 0o700);
        file(zip, "data/noarch/private", b"x", 0o600);
    });
    let staged = stage_bytes(&bytes);
    for (path, mode) in [
        ("setuid", 0o755),
        ("sticky", 0o755),
        ("writable", 0o644),
        ("owner_exec", 0o755),
        ("private", 0o644),
    ] {
        assert_eq!(entry(&staged.entries, path).mode, mode, "{path}");
        assert_eq!(mode_of(&staged.root.join(path)), mode, "{path}");
    }
}

fn split_package(list: &[u8]) -> Vec<u8> {
    installer(|zip| {
        file(zip, "data/noarch/support/postinst.sh", SCRIPT, 0o755);
        file(zip, "data/noarch/support/split_files", list, 0o644);
        // Written out of order, so only sorting gets them right.
        file(zip, "data/noarch/game/big.bin.split10", b"third", 0o644);
        file(zip, "data/noarch/game/big.bin.split01", b"first-", 0o644);
        file(zip, "data/noarch/game/big.bin.split02", b"second-", 0o644);
        file(zip, "data/noarch/other.split-a", b"only", 0o755);
        file(zip, "data/noarch/game/keep.txt", b"keep", 0o644);
    })
}

#[test]
fn split_files_are_assembled_in_byte_order() {
    // Blank and padded lines, and no newline after the last one.
    let bytes = split_package(b"game/big.bin\n\n  other  ");
    let temp = tempfile::tempdir().unwrap();
    let package = open_bytes(temp.path(), &bytes).unwrap();
    assert_eq!(package.splits().len(), 2);
    assert_eq!(
        package.splits()[0].pieces,
        [
            "game/big.bin.split01",
            "game/big.bin.split02",
            "game/big.bin.split10"
        ]
    );
    assert_eq!(package.splits()[1].path, "other");
    assert_eq!(package.assembled_size(), 18 + 4);

    let staged = stage_bytes(&bytes);
    let whole = b"first-second-third";
    assert_eq!(fs::read(staged.root.join("game/big.bin")).unwrap(), whole);
    let big = entry(&staged.entries, "game/big.bin");
    assert_eq!(big.kind, EntryType::File);
    assert_eq!(big.size, Some(whole.len() as u64));
    assert_eq!(big.md5.as_deref(), Some(md5_hex(whole).as_str()));
    assert_eq!(big.mode, 0o644);
    assert_eq!(fs::read(staged.root.join("other")).unwrap(), b"only");
    entry(&staged.entries, "other");
    entry(&staged.entries, "game/keep.txt");
    entry(&staged.entries, "support/postinst.sh");

    let mut on_disk: Vec<String> = fs::read_dir(staged.root.join("game"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    on_disk.sort();
    assert_eq!(on_disk, ["big.bin", "keep.txt"]);
    assert!(!staged.root.join("other.split-a").exists());
    assert!(
        staged
            .entries
            .iter()
            .all(|entry| !entry.path.contains(".split")),
        "{:#?}",
        staged.entries
    );
    let last = staged.progress.last().unwrap();
    assert_eq!(last.done, last.total);
}

#[test]
fn split_files_are_ignored_without_the_known_script() {
    let bytes = installer(|zip| {
        file(zip, "data/noarch/support/split_files", b"big.bin\n", 0o644);
        file(zip, "data/noarch/big.bin.split1", b"a", 0o644);
    });
    let staged = stage_bytes(&bytes);
    assert!(staged.root.join("big.bin.split1").exists());
    assert!(!staged.root.join("big.bin").exists());
}

#[test]
fn unsafe_names_are_refused() {
    for name in [
        "data/noarch/../x",
        "data/noarch//etc/x",
        "data/noarch/a/./b",
        "data/noarch/a\\b",
    ] {
        let detail = refusal(installer(|zip| file(zip, name, b"x", 0o644)));
        assert!(detail.contains("isn't a safe path"), "{name}: {detail}");
    }
}

#[test]
fn entries_beneath_a_symlink_are_refused() {
    let detail = refusal(installer(|zip| {
        dir(zip, "data/noarch/real/");
        link(zip, "data/noarch/link", "real");
        file(zip, "data/noarch/link/file", b"x", 0o644);
    }));
    assert!(detail.contains("beneath the symlink link"), "{detail}");
}

#[test]
fn escaping_symlinks_are_refused() {
    let detail = refusal(installer(|zip| {
        link(zip, "data/noarch/game/up", "../../outside");
    }));
    assert!(detail.contains("up points outside"), "{detail}");

    let detail = refusal(installer(|zip| link(zip, "data/noarch/abs", "/etc")));
    assert!(detail.contains("points outside"), "{detail}");

    // Lexically inside, but `here` is the top, so `here/..` is above it.
    let detail = refusal(installer(|zip| {
        link(zip, "data/noarch/here", ".");
        link(zip, "data/noarch/sneaky", "here/..");
    }));
    assert!(detail.contains("sneaky points outside"), "{detail}");

    // Escaping only through another link that itself escapes.
    let detail = refusal(installer(|zip| {
        dir(zip, "data/noarch/a/b/");
        link(zip, "data/noarch/deep", "a/b");
        link(zip, "data/noarch/via", "deep/../../..");
    }));
    assert!(detail.contains("via points outside"), "{detail}");
}

#[test]
fn symlinks_inside_the_game_are_accepted() {
    let bytes = installer(|zip| {
        dir(zip, "data/noarch/a/b/");
        link(zip, "data/noarch/here", ".");
        link(zip, "data/noarch/deep", "here/a/b/..");
        link(zip, "data/noarch/a/b/up", "../..");
        link(zip, "data/noarch/again", "here/here/here/a");
        link(zip, "data/noarch/dangling", "a/missing");
    });
    let temp = tempfile::tempdir().unwrap();
    open_bytes(temp.path(), &bytes).unwrap();
}

#[test]
fn symlink_cycles_are_refused() {
    let detail = refusal(installer(|zip| {
        link(zip, "data/noarch/a", "b");
        link(zip, "data/noarch/b", "a");
    }));
    assert!(detail.contains("loop"), "{detail}");
    let detail = refusal(installer(|zip| link(zip, "data/noarch/self", "self/x")));
    assert!(detail.contains("loop"), "{detail}");
}

#[test]
fn bad_split_lists_are_refused() {
    for (list, expected) in [
        (&b"../outside\n"[..], "isn't a safe path"),
        (b"/etc/passwd\n", "isn't a safe path"),
        (b"game/missing.bin\n", "game/missing.bin has no pieces"),
        (b"game/big.bin\ngame/big.bin\n", "listed twice"),
        (b"game\n", "game isn't a file"),
    ] {
        let detail = refusal(split_package(list));
        assert!(detail.contains(expected), "{detail}");
    }

    let detail = refusal(installer(|zip| {
        file(zip, "data/noarch/support/postinst.sh", SCRIPT, 0o755);
        file(zip, "data/noarch/support/split_files", b"big", 0o644);
        file(zip, "data/noarch/big.split1", b"x", 0o644);
        link(zip, "data/noarch/big.split2", "big.split1");
    }));
    assert!(detail.contains("big.split2 isn't a file"), "{detail}");
}

#[test]
fn an_unknown_setup_script_is_refused() {
    let bytes = installer(|zip| {
        file(
            zip,
            "data/noarch/support/postinst.sh",
            b"#!/bin/sh\nrm -rf ~\n",
            0o755,
        );
    });
    let temp = tempfile::tempdir().unwrap();
    let error = open_bytes(temp.path(), &bytes).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<PackageError>(),
        Some(PackageError::UnsupportedScript)
    ));
    assert_eq!(
        error.to_string(),
        "This GOG package has a setup script wineo doesn't support."
    );
    // The shipped list doesn't know the test script either.
    let error = open(&save(temp.path(), &split_package(b"game/big.bin"))).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<PackageError>(),
        Some(PackageError::UnsupportedScript)
    ));
}

#[test]
fn special_files_are_refused() {
    let detail = refusal(installer(|zip| {
        raw_mode_file(zip, "data/noarch/pipe", b"", 0o010_644);
    }));
    assert!(
        detail.contains("pipe isn't a file, folder, or symlink"),
        "{detail}"
    );
    let detail = refusal(installer(|zip| {
        raw_mode_file(zip, "data/noarch/disk", b"", 0o060_644);
    }));
    assert!(detail.contains("disk isn't a file"), "{detail}");
}

#[test]
fn duplicate_paths_are_refused() {
    let mut bytes = installer(|zip| {
        file(zip, "data/noarch/same", b"one", 0o644);
        file(zip, "data/noarch/SAME", b"two", 0o644);
    });
    patch(&mut bytes, b"noarch/SAME", b"noarch/same");
    let detail = refusal(bytes);
    assert!(detail.contains("same appears twice"), "{detail}");

    let detail = refusal(installer(|zip| {
        dir(zip, "data/noarch/thing/");
        file(zip, "data/noarch/thing", b"x", 0o644);
    }));
    assert!(detail.contains("thing appears twice"), "{detail}");
}

#[test]
fn non_utf8_names_are_refused() {
    let mut bytes = installer(|zip| file(zip, "data/noarch/name-XX", b"x", 0o644));
    patch(&mut bytes, b"name-XX", b"name-\xff\xfe");
    let detail = refusal(bytes);
    assert!(detail.contains("isn't UTF-8"), "{detail}");
}

#[test]
fn sizes_that_lie_are_refused_when_staging() {
    for (declared, expected) in [
        (5u32, "liar doesn't match its size or checksum"),
        (20, "liar's size doesn't match"),
    ] {
        let mut bytes = installer(|zip| file(zip, "data/noarch/liar", b"0123456789", 0o644));
        // The central directory's uncompressed size, 46 bytes before the name.
        let name = b"data/noarch/liar";
        let at = bytes.windows(name.len()).rposition(|w| w == name).unwrap();
        assert_eq!(&bytes[at - 46..at - 42], &[0x50, 0x4b, 0x01, 0x02]);
        bytes[at - 46 + 24..at - 46 + 28].copy_from_slice(&declared.to_le_bytes());

        let temp = tempfile::tempdir().unwrap();
        let mut package = open_bytes(temp.path(), &bytes).unwrap();
        assert_eq!(package.unpacked_size(), u64::from(declared));
        let folder = temp.path().join("game");
        fs::create_dir(&folder).unwrap();
        let error = stage(
            &mut package,
            &folder,
            "op1",
            1,
            &cancel::Token::new(),
            |_| {},
        )
        .unwrap_err();
        match error.downcast_ref::<PackageError>() {
            Some(PackageError::Invalid(detail)) => {
                assert!(detail.contains(expected), "{detail}")
            }
            other => panic!("expected a size mismatch, got {other:?}: {error:#}"),
        }
    }
}

#[test]
fn an_existing_staging_folder_is_refused() {
    let bytes = installer(|zip| file(zip, "data/noarch/a", b"x", 0o644));
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("game");
    fs::create_dir(&folder).unwrap();
    let elsewhere = temp.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    let mut package = open_bytes(temp.path(), &bytes).unwrap();

    fs::create_dir(folder.join(staging_name("op1", 1))).unwrap();
    stage(
        &mut package,
        &folder,
        "op1",
        1,
        &cancel::Token::new(),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        fs::read_dir(folder.join(staging_name("op1", 1)))
            .unwrap()
            .count(),
        0
    );

    symlink(&elsewhere, folder.join(staging_name("op2", 1))).unwrap();
    stage(
        &mut package,
        &folder,
        "op2",
        1,
        &cancel::Token::new(),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);

    for bad in ["", "a/b", "../x"] {
        stage(&mut package, &folder, bad, 1, &cancel::Token::new(), |_| {}).unwrap_err();
    }
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 2);
}

#[test]
fn canceling_stops_staging_and_remove_staging_cleans_up() {
    let bytes = installer(|zip| {
        file(zip, "data/noarch/one", &[1; 4096], 0o644);
        file(zip, "data/noarch/two", &[2; 4096], 0o644);
        file(zip, "data/noarch/three", &[3; 4096], 0o644);
    });
    let temp = tempfile::tempdir().unwrap();
    let mut package = open_bytes(temp.path(), &bytes).unwrap();
    let folder = temp.path().join("game");
    fs::create_dir(&folder).unwrap();
    let token = cancel::Token::new();
    let error = stage(&mut package, &folder, "op1", 7, &token, |progress| {
        if progress.done >= 4096 {
            token.cancel();
        }
    })
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<PackageError>(),
        Some(PackageError::Canceled)
    ));
    let root = folder.join(staging_name("op1", 7));
    assert!(root.join("one").exists());
    assert!(!root.join("two").exists());

    // Removal never follows a symlink out of the staging folder.
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("precious"), b"keep").unwrap();
    symlink(&outside, root.join("escape")).unwrap();
    fs::create_dir_all(root.join("nested/deeper")).unwrap();
    fs::write(root.join("nested/deeper/file"), b"x").unwrap();

    remove_staging(&folder, "op1", 7).unwrap();
    assert!(!root.exists());
    assert_eq!(fs::read(outside.join("precious")).unwrap(), b"keep");
    remove_staging(&folder, "op1", 7).unwrap();

    // A staging name that's a symlink is removed as a link.
    symlink(&outside, &root).unwrap();
    remove_staging(&folder, "op1", 7).unwrap();
    assert!(fs::symlink_metadata(&root).is_err());
    assert!(outside.join("precious").exists());
}

#[test]
fn space_needed_sums_unpacked_and_assembled_sizes() {
    let temp = tempfile::tempdir().unwrap();
    let plain = open_bytes(
        temp.path(),
        &installer(|zip| file(zip, "data/noarch/a", &[0; 100], 0o644)),
    )
    .unwrap();
    let list = b"game/big.bin\nother\n";
    let split = open_bytes(temp.path(), &split_package(list)).unwrap();
    let split_files = (SCRIPT.len() + list.len()) as u64 + 5 + 6 + 7 + 4 + 4;
    let needed = space_needed(&[&plain, &split]).unwrap();
    assert_eq!(
        needed,
        SpaceNeeded {
            unpacked: 100 + split_files,
            assembled: 18 + 4,
        }
    );
    assert_eq!(needed.total(), 100 + split_files + 22);

    let folder = temp.path().join("game");
    fs::create_dir(&folder).unwrap();
    let paths = Paths::from_lookup(|key| (key == "HOME").then(|| temp.path().into())).unwrap();
    check_space(&paths, &folder, &[&plain, &split]).unwrap();
}

#[test]
fn not_enough_space_names_the_sizes_and_place() {
    let paths =
        Paths::from_lookup(|key| (key == "HOME").then(|| std::ffi::OsString::from("/home/me")))
            .unwrap();
    let needed = SpaceNeeded {
        unpacked: 2_000_000_000,
        assembled: 254_857_830,
    };
    let folder = Path::new("/home/me/Games/rimworld");
    let error = check_space_with(&paths, folder, needed, 1_503_238_553).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Not enough space: needs 2.1 GiB, 1.4 GiB free on ~/Games."
    );
    check_space_with(&paths, folder, needed, needed.total()).unwrap();

    let error = check_space_with(&paths, Path::new("/mnt/games/x"), needed, 0).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Not enough space: needs 2.1 GiB, 0 B free on /mnt/games."
    );
}

#[test]
fn the_shipped_script_list_holds_rimworlds() {
    assert_eq!(
        crate::units::hex(KNOWN_POSTINST[0]),
        "6f91f1bb9bdcc33da5ec5a6279b5e727146ebf2eb83ef9d1b206364a69829756"
    );
}

/// A ZIP64 package whose entries declare these uncompressed sizes in the
/// central directory, whatever they really hold.
fn zip64_declaring(sizes: &[u64]) -> Vec<u8> {
    let names: Vec<String> = (0..sizes.len())
        .map(|n| format!("data/noarch/big{n}"))
        .collect();
    let mut bytes = installer(|zip| {
        for name in &names {
            zip.start_file(name.as_str(), options(0o644).large_file(true))
                .unwrap();
            zip.write_all(b"abc").unwrap();
        }
    });
    for (name, size) in names.iter().zip(sizes) {
        let name = name.as_bytes();
        let at = bytes.windows(name.len()).rposition(|w| w == name).unwrap();
        assert_eq!(&bytes[at - 46..at - 42], &[0x50, 0x4b, 0x01, 0x02]);
        // The ZIP64 extra field follows the name: id 1, length, then the
        // uncompressed size.
        let extra = at + name.len();
        assert_eq!(&bytes[extra..extra + 2], &[0x01, 0x00]);
        bytes[extra + 4..extra + 12].copy_from_slice(&size.to_le_bytes());
    }
    bytes
}

#[test]
fn sizes_that_overflow_are_refused() {
    let detail = refusal(zip64_declaring(&[u64::MAX, 3]));
    assert!(detail.contains("its sizes are too large"), "{detail}");

    let temp = tempfile::tempdir().unwrap();
    let half = 1 << 63;
    let one = open_bytes(temp.path(), &zip64_declaring(&[half])).unwrap();
    assert_eq!(one.unpacked_size(), half);
    let two = open_bytes(temp.path(), &zip64_declaring(&[half])).unwrap();
    let error = space_needed(&[&one, &two]).unwrap_err();
    assert!(matches!(error, PackageError::Invalid(_)), "{error}");
    let folder = temp.path().join("game");
    fs::create_dir(&folder).unwrap();
    let paths = Paths::from_lookup(|key| (key == "HOME").then(|| temp.path().into())).unwrap();
    check_space(&paths, &folder, &[&one, &two]).unwrap_err();
    // One alone is far more than any disk.
    let error = check_space(&paths, &folder, &[&one]).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<PackageError>(),
            Some(PackageError::NoSpace { .. })
        ),
        "{error:#}"
    );
}

/// What a tampering test does to a staged piece just before assembly.
#[derive(Debug, Clone, Copy)]
enum Tamper {
    Grow,
    Shrink,
    Fifo,
    Symlink,
    Folder,
    /// A same-size hard link to a file outside the staging folder.
    HardLink,
}

fn tamper(piece: &Path, how: Tamper) {
    match how {
        Tamper::Grow => {
            let mut file = fs::OpenOptions::new().append(true).open(piece).unwrap();
            file.write_all(b"extra").unwrap();
        }
        Tamper::Shrink => fs::write(piece, b"s").unwrap(),
        Tamper::Fifo => {
            fs::remove_file(piece).unwrap();
            rustix::fs::mknodat(
                rustix::fs::CWD,
                piece,
                rustix::fs::FileType::Fifo,
                rustix::fs::Mode::from_raw_mode(0o644),
                0,
            )
            .unwrap();
        }
        Tamper::Symlink => {
            fs::remove_file(piece).unwrap();
            symlink("big.bin.split01", piece).unwrap();
        }
        Tamper::Folder => {
            fs::remove_file(piece).unwrap();
            fs::create_dir(piece).unwrap();
        }
        Tamper::HardLink => {
            let size = fs::metadata(piece).unwrap().len() as usize;
            let outside = piece.ancestors().nth(4).unwrap().join("outside-secret");
            fs::write(&outside, vec![b'S'; size]).unwrap();
            fs::remove_file(piece).unwrap();
            fs::hard_link(&outside, piece).unwrap();
        }
    }
}

#[test]
fn pieces_changed_before_assembly_are_refused() {
    for how in [
        Tamper::Grow,
        Tamper::Shrink,
        Tamper::Fifo,
        Tamper::Symlink,
        Tamper::Folder,
        Tamper::HardLink,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut package = open_bytes(temp.path(), &split_package(b"game/big.bin")).unwrap();
        let unpacked = package.unpacked_size();
        let folder = temp.path().join("game");
        fs::create_dir(&folder).unwrap();
        let piece = folder
            .join(staging_name("op1", 1))
            .join("game/big.bin.split02");
        let mut done = false;
        let error = stage(
            &mut package,
            &folder,
            "op1",
            1,
            &cancel::Token::new(),
            |progress| {
                // Every file is written and assembly hasn't started.
                if progress.done == unpacked && !done {
                    done = true;
                    tamper(&piece, how);
                }
            },
        )
        .unwrap_err();
        match error.downcast_ref::<PackageError>() {
            Some(PackageError::Invalid(detail)) => {
                assert!(
                    detail.contains("changed while staging"),
                    "{how:?}: {detail}"
                )
            }
            other => panic!("{how:?}: expected a refusal, got {other:?}: {error:#}"),
        }
    }
}

#[test]
fn canceling_during_or_after_assembly_is_honored() {
    // Once during assembly, and once on the very last progress event.
    for during in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let mut package = open_bytes(temp.path(), &split_package(b"game/big.bin")).unwrap();
        let unpacked = package.unpacked_size();
        let folder = temp.path().join("game");
        fs::create_dir(&folder).unwrap();
        let token = cancel::Token::new();
        let error = stage(&mut package, &folder, "op1", 1, &token, |progress| {
            let now = if during {
                progress.done > unpacked
            } else {
                progress.done == progress.total
            };
            if now {
                token.cancel();
            }
        })
        .unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<PackageError>(),
                Some(PackageError::Canceled)
            ),
            "{error:#}"
        );
    }
}
