//! The tree copy engine: reflinks, hardlinks, link rewriting, and staging.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::ioctl_ficlone;
use rustix::io::Errno;

use crate::cancel::Token;
use crate::paths::Paths;

use super::{ImportProgress, check_cancelled, walk_no_follow};

pub(crate) fn copy_tree(
    source: &Path,
    destination: &Path,
    final_root: &Path,
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<()> {
    let links = Links::default();
    copy_tree_with_reflink_mode(
        source,
        destination,
        final_root,
        &links,
        cancel,
        progress,
        false,
    )
}

/// How a copy rewrites links that point inside what is being copied.
#[derive(Debug, Default)]
pub(crate) struct Links {
    /// Other trees copied alongside, as `(from, to)`. A link into one follows
    /// it the way a link into the copied tree itself does.
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// Rewritten links are written relative to where they sit, so a portable
    /// folder can move. Otherwise they are absolute.
    pub relative: bool,
}

impl Links {
    /// Relative links when the copy lands in a portable folder.
    pub(crate) fn for_paths(paths: &Paths) -> Self {
        Self {
            moved: Vec::new(),
            relative: paths.portable_root().is_some(),
        }
    }
}

pub(crate) fn copy_tree_with(
    source: &Path,
    destination: &Path,
    final_root: &Path,
    links: &Links,
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<()> {
    copy_tree_with_reflink_mode(
        source,
        destination,
        final_root,
        links,
        cancel,
        progress,
        false,
    )
}

#[cfg(test)]
pub(crate) fn copy_tree_strict_reflink(
    source: &Path,
    destination: &Path,
    final_root: &Path,
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<()> {
    let links = Links::default();
    copy_tree_with_reflink_mode(
        source,
        destination,
        final_root,
        &links,
        cancel,
        progress,
        true,
    )
}

/// `copy_tree_strict_reflink` with the link handling `links` asks for.
pub(crate) fn copy_tree_strict_reflink_with(
    source: &Path,
    destination: &Path,
    final_root: &Path,
    links: &Links,
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
) -> Result<()> {
    copy_tree_with_reflink_mode(
        source,
        destination,
        final_root,
        links,
        cancel,
        progress,
        true,
    )
}

fn copy_tree_with_reflink_mode(
    source: &Path,
    destination: &Path,
    final_root: &Path,
    links: &Links,
    cancel: &Token,
    progress: &mut impl FnMut(ImportProgress),
    strict_reflink: bool,
) -> Result<()> {
    // Only the count is needed here, so skip reading file content.
    let mut total = 0;
    walk_no_follow(source, &mut |_, _| {
        check_cancelled(cancel)?;
        total += 1;
        Ok(())
    })?;
    progress(ImportProgress::Started { entries: total });
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    let mut hardlinks = HashMap::new();
    let options = CopyOptions {
        source_root: source,
        final_root,
        links,
        cancel,
        strict_reflink,
    };
    copy_entry(source, destination, &options, progress, &mut hardlinks)?;
    Ok(())
}

struct CopyOptions<'a> {
    source_root: &'a Path,
    final_root: &'a Path,
    links: &'a Links,
    cancel: &'a Token,
    strict_reflink: bool,
}

fn copy_entry(
    source: &Path,
    destination: &Path,
    options: &CopyOptions<'_>,
    progress: &mut impl FnMut(ImportProgress),
    hardlinks: &mut HashMap<(u64, u64), PathBuf>,
) -> Result<()> {
    check_cancelled(options.cancel)?;
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect {}", source.display()))?;
    let kind = metadata.file_type();
    if kind.is_symlink() {
        let target = fs::read_link(source)
            .with_context(|| format!("failed to read link {}", source.display()))?;
        let resolved = resolve_link(source, &target);
        let rewrite = |to: PathBuf| {
            if !options.links.relative {
                return to;
            }
            // Relative to where the link ends up, not where it is staged.
            let inside = source
                .strip_prefix(options.source_root)
                .unwrap_or(Path::new(""));
            let placed = options.final_root.join(inside);
            relative_path(&to, placed.parent().unwrap_or(Path::new("/")))
        };
        let target = if resolved.starts_with(options.source_root) {
            if target.is_absolute() {
                let relative = resolved
                    .strip_prefix(options.source_root)
                    .unwrap_or(Path::new(""));
                rewrite(lexical_normalize(&options.final_root.join(relative)))
            } else {
                target
            }
        } else if let Some((from, to)) = options
            .links
            .moved
            .iter()
            .find(|(from, _)| resolved.starts_with(from))
        {
            let relative = resolved.strip_prefix(from).unwrap_or(Path::new(""));
            rewrite(lexical_normalize(&to.join(relative)))
        } else {
            // An external link would point somewhere else after copying when
            // it is relative, or through an internal link that is re-rooted.
            // Keep the resolved external data location explicit.
            resolved
        };
        std::os::unix::fs::symlink(&target, destination)
            .with_context(|| format!("failed to copy link {}", source.display()))?;
    } else if kind.is_dir() {
        if source != options.source_root {
            fs::create_dir(destination)
                .with_context(|| format!("failed to create {}", destination.display()))?;
        }
        let mut children: Vec<PathBuf> = fs::read_dir(source)
            .with_context(|| format!("failed to read {}", source.display()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<_>>()
            .with_context(|| format!("failed to read {}", source.display()))?;
        children.sort();
        for child in children {
            let name = child
                .file_name()
                .ok_or_else(|| anyhow!("path has no file name: {}", child.display()))?;
            copy_entry(
                &child,
                &destination.join(name),
                options,
                progress,
                hardlinks,
            )?;
        }
        copy_metadata(&metadata, destination)?;
    } else if kind.is_file() {
        let key = (metadata.dev(), metadata.ino());
        if let Some(first) = hardlinks.get(&key) {
            fs::hard_link(first, destination).with_context(|| {
                format!("failed to preserve hardlink {}", destination.display())
            })?;
        } else {
            if options.strict_reflink {
                copy_regular_file_strict(source, destination)?;
            } else {
                copy_regular_file(source, destination)?;
            }
            hardlinks.insert(key, destination.to_owned());
        }
        copy_metadata(&metadata, destination)?;
    } else {
        bail!("unsupported special file in prefix: {}", source.display());
    }
    progress(ImportProgress::Copied {
        path: source.to_owned(),
    });
    Ok(())
}

fn copy_regular_file(source: &Path, destination: &Path) -> Result<()> {
    copy_regular_file_mode(source, destination, false)
}

fn copy_regular_file_strict(source: &Path, destination: &Path) -> Result<()> {
    copy_regular_file_mode(source, destination, true)
}

#[cfg(test)]
type Reflink = Box<dyn FnOnce(&mut File, &mut File) -> rustix::io::Result<()>>;

#[cfg(test)]
thread_local! {
    /// Stands in for the next reflink on this thread.
    static FAKE_REFLINK: std::cell::Cell<Option<Reflink>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(super) fn copy_regular_file_with(
    source: &Path,
    destination: &Path,
    reflink: impl FnOnce(&mut File, &mut File) -> rustix::io::Result<()> + 'static,
) -> Result<()> {
    copy_regular_file_with_mode(source, destination, reflink, false)
}

#[cfg(test)]
pub(super) fn copy_regular_file_with_mode(
    source: &Path,
    destination: &Path,
    reflink: impl FnOnce(&mut File, &mut File) -> rustix::io::Result<()> + 'static,
    strict_reflink: bool,
) -> Result<()> {
    FAKE_REFLINK.with(|fake| fake.set(Some(Box::new(reflink))));
    let result = copy_regular_file_mode(source, destination, strict_reflink);
    FAKE_REFLINK.with(|fake| fake.set(None));
    result
}

fn reflink(output: &mut File, input: &mut File) -> rustix::io::Result<()> {
    #[cfg(test)]
    if let Some(fake) = FAKE_REFLINK.with(std::cell::Cell::take) {
        return fake(output, input);
    }
    ioctl_ficlone(output, input)
}

fn copy_regular_file_mode(source: &Path, destination: &Path, strict_reflink: bool) -> Result<()> {
    let mut input =
        File::open(source).with_context(|| format!("failed to open {}", source.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    match reflink(&mut output, &mut input) {
        Ok(()) => Ok(()),
        Err(err)
            if strict_reflink
                && matches!(
                    err,
                    Errno::OPNOTSUPP | Errno::NOTTY | Errno::XDEV | Errno::INVAL | Errno::NOSYS
                ) =>
        {
            Err(anyhow!(err)).with_context(|| {
                format!(
                    "filesystem does not support reflinks from {} to {}",
                    source.display(),
                    destination.display()
                )
            })
        }
        Err(Errno::OPNOTSUPP | Errno::NOTTY | Errno::XDEV | Errno::INVAL | Errno::NOSYS) => {
            fs::copy(source, destination)
                .with_context(|| format!("failed to copy {}", source.display()))?;
            Ok(())
        }
        Err(err) => Err(anyhow!(err)).with_context(|| {
            format!(
                "failed to reflink {} to {}",
                source.display(),
                destination.display()
            )
        }),
    }
}

fn copy_metadata(source: &fs::Metadata, destination: &Path) -> Result<()> {
    fs::set_permissions(destination, fs::Permissions::from_mode(source.mode()))
        .with_context(|| format!("failed to preserve mode on {}", destination.display()))?;
    let modified = filetime::FileTime::from_last_modification_time(source);
    let accessed = filetime::FileTime::from_last_access_time(source);
    filetime::set_file_times(destination, accessed, modified)
        .with_context(|| format!("failed to preserve timestamps on {}", destination.display()))?;
    Ok(())
}

pub(super) fn resolve_link(source: &Path, target: &Path) -> PathBuf {
    let base = if target.is_absolute() {
        target.to_owned()
    } else {
        source.parent().unwrap_or(Path::new(".")).join(target)
    };
    let lexical = lexical_normalize(&base);
    fs::canonicalize(&lexical).unwrap_or(lexical)
}

/// `target` as seen from the directory `base`, both absolute and normalized.
pub(super) fn relative_path(target: &Path, base: &Path) -> PathBuf {
    let target: Vec<Component> = target.components().collect();
    let base: Vec<Component> = base.components().collect();
    let common = target
        .iter()
        .zip(&base)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..base.len() {
        relative.push("..");
    }
    for component in &target[common..] {
        relative.push(component.as_os_str());
    }
    if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative
    }
}

pub(super) fn lexical_normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            Component::RootDir | Component::Prefix(_) => result.push(component.as_os_str()),
            Component::Normal(part) => result.push(part),
        }
    }
    result
}

/// A staging directory for a copied tree. A copy that stops part way can
/// leave read-only directories, which `TempDir` cleanup can't empty, so they
/// are made writable first.
pub(crate) struct Staging(Option<tempfile::TempDir>);

impl Staging {
    pub(crate) fn new_in(prefix: &str, parent: impl AsRef<Path>) -> io::Result<Self> {
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(parent)
            .map(|dir| Self(Some(dir)))
    }

    pub(crate) fn path(&self) -> &Path {
        self.0
            .as_ref()
            .expect("staging is held until dropped")
            .path()
    }

    /// Hands the directory over, so it is no longer removed on drop.
    pub(crate) fn keep(mut self) -> PathBuf {
        self.0.take().expect("staging is held until dropped").keep()
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            #[cfg(test)]
            ARMED_DROPS.with(|drops| drops.borrow_mut().push(dir.path().to_owned()));
            make_removable(dir.path());
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Every staging directory this thread dropped while it was still
    /// armed to be removed, in order.
    pub(crate) static ARMED_DROPS: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Best effort, since cleanup reports nothing either.
fn make_removable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if !metadata.is_dir() {
        return;
    }
    let mode = metadata.permissions().mode();
    if mode & 0o700 != 0o700 {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o700));
    }
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            make_removable(&entry.path());
        }
    }
}

/// Flushes the filesystem holding `path`, so a copied tree and its rename
/// reach the disk before a config naming it does. One call covers every
/// file, which is cheaper than syncing each as it is copied.
pub(crate) fn sync_filesystem(path: &Path) -> Result<()> {
    let dir = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    rustix::fs::syncfs(&dir).with_context(|| format!("failed to sync {}", path.display()))
}
