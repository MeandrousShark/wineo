//! Test-only: points a copied trash entry's recorded identities at the copy.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use toml_edit::{Array, DocumentMut, Item, Value};

use crate::paths::Paths;

use super::identity::identity;
use super::{Identity, MANIFEST, Manifest, Phase, Step, resolved};

/// Rewrites the device, inode, mount, and birth time in every identity table
/// of every manifest under the trash directory to what the files have now, and
/// touches nothing else in the file. A trash entry records the identity of the
/// directories it moved things into, so a manifest copied from another
/// checkout never matches. Returns how many manifests it rewrote. Entries that
/// are mid-operation (any phase but trashed, any step but retained, or a
/// staged or copied payload) are refused.
///
/// Fixture tests use this on copies of what an old release wrote. The check
/// that makes it necessary stays strict in production.
#[doc(hidden)]
pub fn rebind_identities(paths: &Paths) -> Result<usize> {
    let root = paths.trash_dir();
    let mut count = 0;
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err).with_context(|| format!("reading {}", root.display())),
    };
    for entry in entries {
        let dir = entry?.path();
        let file = dir.join(MANIFEST);
        if !file.is_file() {
            continue;
        }
        rebind_manifest(paths, &dir, &file)?;
        count += 1;
    }
    Ok(count)
}

fn rebind_manifest(paths: &Paths, dir: &Path, file: &Path) -> Result<()> {
    let text = fs::read_to_string(file)?;
    let manifest: Manifest = toml::from_str(&text)?;
    let mut doc: DocumentMut = text.parse()?;

    // Only what a finished removal leaves behind: anything else is an
    // operation in progress, whose identities mean something else.
    if manifest.phase != Phase::Trashed {
        bail!("trash entry {} is mid-operation", manifest.id);
    }

    set(&mut doc["entry_identity"], dir)?;
    if manifest.prefix_identity.is_some() {
        let reference = manifest
            .prefix_reference
            .as_deref()
            .ok_or_else(|| anyhow!("prefix identity without a prefix"))?;
        let prefix = if manifest.include_prefix {
            let payload = manifest
                .payloads
                .iter()
                .find(|payload| payload.kind == super::Kind::Prefix)
                .ok_or_else(|| anyhow!("prefix in the trash without a payload"))?;
            resolved(paths, &payload.retained)?
        } else {
            resolved(paths, reference)?
        };
        set(&mut doc["prefix_identity"], &prefix)?;
    }
    let tables = doc["payloads"]
        .as_array_of_tables_mut()
        .ok_or_else(|| anyhow!("manifest has no payloads"))?;
    for (table, payload) in tables.iter_mut().zip(&manifest.payloads) {
        if payload.step != Step::Retained
            || payload.kept
            || payload.copied_identity.is_some()
            || payload.returned_identity.is_some()
            || payload.out_stage.is_some()
            || payload.return_stage.is_some()
        {
            bail!("a mid-operation payload can't be rebound");
        }
        let retained = resolved(paths, &payload.retained)?;
        let original = resolved(paths, &payload.original)?;
        let parent_of = |path: &Path| {
            path.parent()
                .map(Path::to_owned)
                .ok_or_else(|| anyhow!("no parent: {}", path.display()))
        };
        set(&mut table["identity"], &retained)?;
        set(&mut table["source_parent"], &parent_of(&original)?)?;
        set(&mut table["destination_parent"], &parent_of(&retained)?)?;
    }
    fs::write(file, doc.to_string())?;
    Ok(())
}

/// Replaces the numbers in one identity table, keeping its other formatting.
fn set(item: &mut Item, path: &Path) -> Result<()> {
    let now: Identity = identity(path)?.ok_or_else(|| anyhow!("{} is missing", path.display()))?;
    let table = item
        .as_table_like_mut()
        .ok_or_else(|| anyhow!("identity is not a table"))?;
    put(table.get_mut("device"), now.device as i64)?;
    put(table.get_mut("inode"), now.inode as i64)?;
    if let Some(mount) = now.mount
        && table.contains_key("mount")
    {
        put(table.get_mut("mount"), mount as i64)?;
    }
    if let Some(fsid) = now.fsid
        && table.contains_key("fsid")
    {
        // A u64 ID can exceed TOML's i64, so it is stored as its bit pattern.
        put(table.get_mut("fsid"), fsid as i64)?;
    }
    let Some((secs, nanos)) = now.birth else {
        bail!("{} has no birth time on this filesystem", path.display());
    };
    let birth = table
        .get_mut("birth")
        .and_then(Item::as_array_mut)
        .ok_or_else(|| anyhow!("identity has no birth time"))?;
    replace(birth, 0, secs)?;
    replace(birth, 1, i64::from(nanos))
}

fn put(item: Option<&mut Item>, number: i64) -> Result<()> {
    let item = item.ok_or_else(|| anyhow!("identity field missing"))?;
    let decor = item.as_value().map(|value| value.decor().clone());
    let mut value = Value::from(number);
    if let Some(decor) = decor {
        *value.decor_mut() = decor;
    }
    *item = Item::Value(value);
    Ok(())
}

fn replace(array: &mut Array, index: usize, number: i64) -> Result<()> {
    let old = array
        .get(index)
        .ok_or_else(|| anyhow!("birth time is too short"))?;
    let mut value = Value::from(number);
    *value.decor_mut() = old.decor().clone();
    array.replace(index, value);
    Ok(())
}
