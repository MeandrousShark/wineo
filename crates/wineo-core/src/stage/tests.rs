use std::cell::Cell;
use std::process::Command;

use super::*;
use crate::import::{ARMED_DROPS, Cancelled};

fn prefix(root: &Path) {
    fs::create_dir_all(root.join("drive_c/game")).unwrap();
    fs::write(root.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    fs::write(root.join("drive_c/game/save.dat"), "save").unwrap();
}

fn copy<'a>(source: &'a Path, destination: &'a Path, links: &'a Links) -> PrefixCopy<'a> {
    PrefixCopy {
        source,
        destination,
        layout: ImportLayout::Wine,
        links,
        staging_prefix: ".wineo-test-",
        label: "test",
    }
}

/// Everything in `dir` except the destination's install lock file.
fn left_behind(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with(".wineo-lock-"))
        .collect();
    names.sort();
    names
}

#[test]
fn publishes_after_verification() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    prefix(&source);
    let parent = temp.path().join("prefixes");
    let destination = parent.join("game");
    let links = Links::default();
    let verified = Cell::new(false);
    let mut events = Vec::new();
    let _lock = copy(&source, &destination, &links)
        .run(&Token::new(), &mut |event| events.push(event), |_| {
            assert!(!destination.exists());
            let staged = left_behind(&parent);
            assert_eq!(staged.len(), 1);
            assert!(staged[0].starts_with(".wineo-test-"));
            assert!(parent.join(&staged[0]).join("system.reg").is_file());
            verified.set(true);
            Ok(())
        })
        .unwrap();
    assert!(verified.get());
    assert_eq!(
        fs::read_to_string(destination.join("drive_c/game/save.dat")).unwrap(),
        "save"
    );
    assert_eq!(left_behind(&parent), ["game"]);
    assert!(matches!(events[0], ImportProgress::Started { .. }));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ImportProgress::Finished { .. }))
    );
}

#[test]
fn verification_failure_leaves_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    prefix(&source);
    let parent = temp.path().join("prefixes");
    let destination = parent.join("game");
    let links = Links::default();
    let err = copy(&source, &destination, &links)
        .run(&Token::new(), &mut |_| {}, |_| bail!("source changed"))
        .err()
        .expect("the copy fails");
    assert_eq!(err.to_string(), "source changed");
    assert!(!destination.exists());
    assert!(left_behind(&parent).is_empty());
}

#[test]
fn copy_error_leaves_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    prefix(&source);
    // A read-only directory checks that cleanup can still empty it.
    fs::create_dir(source.join("drive_c/locked")).unwrap();
    fs::write(source.join("drive_c/locked/file"), "x").unwrap();
    let status = Command::new("mkfifo")
        .arg(source.join("drive_c/zz-fifo"))
        .status()
        .unwrap();
    assert!(status.success());
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        source.join("drive_c/locked"),
        fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    let parent = temp.path().join("prefixes");
    let destination = parent.join("game");
    let links = Links::default();
    let err = copy(&source, &destination, &links)
        .run(&Token::new(), &mut |_| {}, |_| {
            panic!("verify must not run after a failed copy")
        })
        .err()
        .expect("the copy fails");
    fs::set_permissions(
        source.join("drive_c/locked"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(
        err.to_string().contains("unsupported special file"),
        "{err}"
    );
    assert!(!destination.exists());
    assert!(left_behind(&parent).is_empty());
}

#[test]
fn cancel_leaves_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    prefix(&source);
    let parent = temp.path().join("prefixes");
    let destination = parent.join("game");
    let links = Links::default();
    let cancel = Token::new();
    let err = copy(&source, &destination, &links)
        .run(
            &cancel,
            &mut |event| {
                if matches!(event, ImportProgress::Copied { .. }) {
                    cancel.cancel();
                }
            },
            |_| panic!("verify must not run after a cancel"),
        )
        .err()
        .expect("the copy fails");
    assert!(err.is::<Cancelled>(), "{err}");
    assert!(!destination.exists());
    assert!(left_behind(&parent).is_empty());
}

#[test]
fn cancel_during_verification_leaves_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    prefix(&source);
    let parent = temp.path().join("prefixes");
    let destination = parent.join("game");
    let links = Links::default();
    let cancel = Token::new();
    let err = copy(&source, &destination, &links)
        .run(&cancel, &mut |_| {}, |_| {
            cancel.cancel();
            import::check_cancelled(&cancel)
        })
        .err()
        .expect("the copy fails");
    assert!(err.is::<Cancelled>(), "{err}");
    assert!(!destination.exists());
    assert!(left_behind(&parent).is_empty());
}

#[test]
fn invalid_staged_prefix_is_not_published() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir_all(source.join("drive_c")).unwrap();
    fs::create_dir(source.join("system.reg")).unwrap();
    let parent = temp.path().join("prefixes");
    let destination = parent.join("game");
    let links = Links::default();
    let err = copy(&source, &destination, &links)
        .run(&Token::new(), &mut |_| {}, |_| Ok(()))
        .err()
        .expect("the copy fails");
    assert_eq!(
        err.to_string(),
        "staged import is missing system.reg or drive_c"
    );
    assert!(!destination.exists());
    assert!(left_behind(&parent).is_empty());
}

#[test]
fn destination_appearing_is_not_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    prefix(&source);
    let parent = temp.path().join("prefixes");
    let destination = parent.join("game");
    let links = Links::default();
    let err = copy(&source, &destination, &links)
        .run(&Token::new(), &mut |_| {}, |_| {
            fs::create_dir(&destination).unwrap();
            fs::write(destination.join("theirs"), "keep").unwrap();
            Ok(())
        })
        .err()
        .expect("the copy fails");
    assert_eq!(
        err.to_string(),
        format!(
            "test destination appeared during publication: {}",
            destination.display()
        )
    );
    assert_eq!(
        fs::read_to_string(destination.join("theirs")).unwrap(),
        "keep"
    );
    assert_eq!(left_behind(&parent), ["game"]);
}

#[test]
fn existing_destination_is_refused_before_staging() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    prefix(&source);
    let parent = temp.path().join("prefixes");
    let destination = parent.join("game");
    fs::create_dir_all(&destination).unwrap();
    let links = Links::default();
    let err = copy(&source, &destination, &links)
        .run(&Token::new(), &mut |_| {}, |_| Ok(()))
        .err()
        .expect("the copy fails");
    assert_eq!(
        err.to_string(),
        format!(
            "test destination appeared during review: {}",
            destination.display()
        )
    );
    assert_eq!(left_behind(&parent), ["game"]);
}

#[test]
fn unpublished_parts_are_removed_on_drop() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("prefixes");
    let stage = Stage::new(".wineo-test-", &parent).unwrap();
    fs::create_dir(stage.path().join("prefix")).unwrap();
    fs::create_dir(stage.path().join("snapshots")).unwrap();
    let destination = parent.join("game");
    stage
        .publish_part(&stage.path().join("prefix"), &destination)
        .unwrap();
    drop(stage);
    assert_eq!(left_behind(&parent), ["game"]);
}

fn armed_drops() -> Vec<PathBuf> {
    ARMED_DROPS.with(|drops| drops.borrow().clone())
}

// The staging path is gone after the rename, so an armed guard would
// remove nothing today. Disarming keeps a later cleanup change from ever
// reaching what is now the user's prefix.
#[test]
fn published_stage_is_disarmed() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("prefixes");
    let stage = Stage::new(".wineo-test-", &parent).unwrap();
    let staging = stage.path().to_owned();
    fs::write(staging.join("system.reg"), "keep").unwrap();
    let destination = parent.join("game");
    stage.publish(&destination).unwrap();
    assert!(!armed_drops().contains(&staging));
    assert_eq!(
        fs::read_to_string(destination.join("system.reg")).unwrap(),
        "keep"
    );
    assert_eq!(left_behind(&parent), ["game"]);
}

#[test]
fn prefix_copy_disarms_after_publishing() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    prefix(&source);
    let parent = temp.path().join("prefixes");
    let destination = parent.join("game");
    let links = Links::default();
    let before = armed_drops().len();
    let _lock = copy(&source, &destination, &links)
        .run(&Token::new(), &mut |_| {}, |_| Ok(()))
        .unwrap();
    assert_eq!(armed_drops().len(), before);
    assert!(destination.join("system.reg").is_file());
}

#[test]
fn failed_rename_onto_existing_destination_cleans_up() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("prefixes");
    let stage = Stage::new(".wineo-test-", &parent).unwrap();
    let staging = stage.path().to_owned();
    fs::write(staging.join("system.reg"), "staged").unwrap();
    let destination = parent.join("game");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("theirs"), "keep").unwrap();
    let err = stage.publish(&destination).unwrap_err();
    assert!(
        err.to_string().contains("cannot publish without replacing"),
        "{err}"
    );
    assert!(armed_drops().contains(&staging));
    assert_eq!(
        fs::read_to_string(destination.join("theirs")).unwrap(),
        "keep"
    );
    assert_eq!(left_behind(&destination), ["theirs"]);
    assert_eq!(left_behind(&parent), ["game"]);
}

#[test]
fn failed_rename_into_read_only_parent_cleans_up() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("prefixes");
    let stage = Stage::new(".wineo-test-", &parent).unwrap();
    fs::write(stage.path().join("system.reg"), "staged").unwrap();
    let locked = temp.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let destination = locked.join("game");
    let result = stage.publish(&destination);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("cannot publish without replacing"),
        "{err}"
    );
    assert!(!destination.exists());
    assert!(left_behind(&parent).is_empty());
}

#[test]
fn panic_in_verify_cleans_up() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    prefix(&source);
    let parent = temp.path().join("prefixes");
    let destination = parent.join("game");
    let links = Links::default();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        copy(&source, &destination, &links)
            .run(&Token::new(), &mut |_| {}, |_| panic!("verify failed"))
    }));
    assert!(panicked.is_err());
    assert!(!destination.exists());
    assert!(left_behind(&parent).is_empty());
}
