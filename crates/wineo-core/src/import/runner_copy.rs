//! Copying an existing runner tree into a runner root, with a provenance receipt.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::cancel::Token;
use crate::download::{InstallLock, publish};
use crate::name;
use crate::runner_alias;

use super::copy::{Staging, copy_tree, lexical_normalize, sync_filesystem};
use super::fingerprint::{Check, FingerprintKind, import_fingerprint, scan};
use super::{
    ExternalDependency, ImportProgress, RunnerCopyInspection, RunnerCopyReceipt, RunnerProvenance,
    canonical_source, check_cancelled, reject_overlapping_paths,
};

/// Copy an existing runner tree into a wineo runner root without replacing an
/// installed build. The copy is staged and receives an import provenance file.
pub fn inspect_runner_copy(source: impl AsRef<Path>) -> Result<RunnerCopyInspection> {
    let source = canonical_source(source.as_ref())?;
    let (supports_wine, supports_proton) = runner_tree_kinds(&source)?;
    if !supports_wine && !supports_proton {
        bail!(
            "{} is not a valid Wine or Proton runner tree",
            source.display()
        );
    }
    let cancel = Token::new();
    let (external_dependencies, fingerprint) = scan(
        &source,
        FingerprintKind::Metadata,
        &mut Check::new(&cancel, &mut |_| {}),
    )?;
    Ok(RunnerCopyInspection {
        external_dependencies,
        fingerprint,
        source,
        supports_wine,
        supports_proton,
    })
}

/// Copy the same runner tree that was offered in an earlier review.
pub fn copy_runner_reviewed(
    reviewed: &RunnerCopyInspection,
    destination_root: impl AsRef<Path>,
    name: &str,
    cancel: &Token,
    progress: impl FnMut(ImportProgress),
) -> Result<RunnerCopyReceipt> {
    copy_runner_with_review(
        &reviewed.source,
        destination_root.as_ref(),
        name,
        Some(reviewed),
        cancel,
        progress,
    )
}

pub(super) fn copy_runner_with_review(
    source: &Path,
    destination_root: &Path,
    name: &str,
    reviewed: Option<&RunnerCopyInspection>,
    cancel: &Token,
    mut progress: impl FnMut(ImportProgress),
) -> Result<RunnerCopyReceipt> {
    let inspection = inspect_runner_copy(source)?;
    if reviewed.is_some_and(|reviewed| reviewed != &inspection) {
        bail!("the reviewed runner changed; inspect it again before copying");
    }
    let source = &inspection.source;
    if !name::is_valid_runner(name) {
        bail!("invalid runner name `{name}`");
    }
    if runner_alias::is_alias(name) {
        bail!("runner name `{name}` is reserved for a latest alias");
    }
    let destination_root = std::path::absolute(destination_root)?;
    fs::create_dir_all(&destination_root)
        .with_context(|| format!("failed to create {}", destination_root.display()))?;
    let destination = destination_root.join(name);
    reject_overlapping_paths(source, &destination)?;
    if destination.exists() {
        bail!(
            "runner destination already exists: {}",
            destination.display()
        );
    }
    let before = inspection.fingerprint;
    let _publish_lock = InstallLock::acquire(&destination)?;
    let staging = Staging::new_in(".wineo-runner-", &destination_root).with_context(|| {
        format!(
            "failed to create staging directory in {}",
            destination_root.display()
        )
    })?;
    let staging_path = staging.path().to_owned();
    copy_tree(source, &staging_path, &destination, cancel, &mut progress)?;
    check_cancelled(cancel)?;
    if import_fingerprint(source, &mut Check::new(cancel, &mut |_| {}))? != before {
        bail!(
            "runner {} changed while it was being copied; try again",
            source.display()
        );
    }
    validate_staged_runner_tree(&staging_path, &destination)?;
    let receipt = staging_path.join(".wineo-install.json");
    if let Ok(metadata) = fs::symlink_metadata(&receipt) {
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            bail!(
                "runner source has a non-regular reserved receipt: {}",
                receipt.display()
            );
        }
        fs::remove_file(&receipt)
            .with_context(|| format!("failed to replace {}", receipt.display()))?;
    }
    let provenance = runner_provenance(source, name)?;
    let receipt_json = runner_receipt(&provenance)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt)
        .and_then(|mut file| file.write_all(&receipt_json))
        .with_context(|| format!("failed to write {}", receipt.display()))?;
    if destination.exists() {
        bail!(
            "runner destination appeared during publication: {}",
            destination.display()
        );
    }
    publish(&staging_path, &destination)?;
    sync_filesystem(&destination)?;
    Ok(RunnerCopyReceipt {
        path: destination.clone(),
        name: name.to_owned(),
        receipt: destination.join(".wineo-install.json"),
        provenance,
        external_dependencies: inspection
            .external_dependencies
            .into_iter()
            .map(|dependency| ExternalDependency {
                source: destination.join(dependency.source.strip_prefix(source).unwrap()),
                target: dependency.target,
            })
            .collect(),
    })
}

fn runner_provider(name: &str) -> String {
    if name.starts_with("GE-Proton") {
        "ge-proton".to_owned()
    } else if name.to_ascii_lowercase().starts_with("proton-cachyos-")
        || name.to_ascii_lowercase().starts_with("cachyos-")
    {
        "proton-cachyos".to_owned()
    } else {
        "import".to_owned()
    }
}

fn runner_provenance(source: &Path, name: &str) -> Result<RunnerProvenance> {
    let reported = fs::read_to_string(source.join("version"))
        .ok()
        .and_then(|text| text.split_whitespace().nth(1).map(str::to_owned));
    let version = reported.unwrap_or_else(|| name.to_owned());
    let provider = runner_provider(&version);
    let (provider, variant) = if provider == "import" {
        (runner_provider(name), "baseline".to_owned())
    } else if provider == "proton-cachyos" {
        (provider, "slr".to_owned())
    } else {
        (provider, "baseline".to_owned())
    };
    Ok(RunnerProvenance {
        stable: stable_runner_version(&version, &provider),
        provider,
        source: source.to_owned(),
        method: "copy".to_owned(),
        version,
        architecture: "x86_64".to_owned(),
        variant,
    })
}

fn stable_runner_version(version: &str, provider: &str) -> bool {
    let parts: Option<Vec<&str>> = if provider == "ge-proton" {
        version
            .strip_prefix("GE-Proton")
            .map(|rest| rest.split('-').collect())
    } else if provider == "proton-cachyos" {
        version
            .strip_prefix("cachyos-")
            .or_else(|| version.strip_prefix("proton-cachyos-"))
            .and_then(|rest| rest.strip_suffix("-slr"))
            .map(|rest| rest.split(['.', '-']).collect())
    } else {
        None
    };
    let Some(parts) = parts else {
        return false;
    };
    let count = if provider == "ge-proton" { 2 } else { 3 };
    parts.len() == count
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn runner_receipt(provenance: &RunnerProvenance) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(&serde_json::json!({
        "provider": provenance.provider,
        "version": provenance.version,
        "architecture": provenance.architecture,
        "variant": provenance.variant,
        "stable": provenance.stable,
        "asset": "",
        "digest": "",
        "verified_digests": [],
        "method": provenance.method,
        "source": provenance.source,
    }))?)
}

fn runner_tree_kinds(root: &Path) -> Result<(bool, bool)> {
    let proton = root.join("proton");
    let wine = root.join("bin/wine");
    let wineserver = root.join("bin/wineserver");
    let proton_ok = proton.is_file()
        && proton.metadata()?.permissions().mode() & 0o111 != 0
        && ["files", "dist"].into_iter().any(|payload| {
            let wine = root.join(payload).join("bin/wine");
            let wineserver = root.join(payload).join("bin/wineserver");
            wine.is_file()
                && wineserver.is_file()
                && wine
                    .metadata()
                    .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
                && wineserver
                    .metadata()
                    .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
        });
    let wine_ok = wine.is_file()
        && wineserver.is_file()
        && wine.metadata()?.permissions().mode() & 0o111 != 0
        && wineserver.metadata()?.permissions().mode() & 0o111 != 0;
    Ok((wine_ok, proton_ok))
}

fn validate_staged_runner_tree(staging: &Path, final_root: &Path) -> Result<()> {
    let proton = staged_path_if_exists(&staging.join("proton"), staging, final_root)?;
    let wine = staged_path_if_exists(&staging.join("bin/wine"), staging, final_root)?;
    let wineserver = staged_path_if_exists(&staging.join("bin/wineserver"), staging, final_root)?;
    let mut proton_payload = false;
    for payload in ["files", "dist"] {
        if staged_executable_if_exists(
            &staging.join(payload).join("bin/wine"),
            staging,
            final_root,
        )? && staged_executable_if_exists(
            &staging.join(payload).join("bin/wineserver"),
            staging,
            final_root,
        )? {
            proton_payload = true;
            break;
        }
    }
    let proton_ok = match proton.as_ref() {
        Some(path) => path.is_file() && executable_metadata(path)?,
        None => false,
    } && proton_payload;
    let wine_ok = match (wine.as_ref(), wineserver.as_ref()) {
        (Some(wine), Some(wineserver)) => {
            wine.is_file()
                && wineserver.is_file()
                && executable_metadata(wine)?
                && executable_metadata(wineserver)?
        }
        _ => false,
    };
    if !proton_ok && !wine_ok {
        bail!(
            "{} is not a valid Wine or Proton runner tree",
            staging.display()
        );
    }
    Ok(())
}

pub(super) fn staged_path_if_exists(
    path: &Path,
    staging: &Path,
    final_root: &Path,
) -> Result<Option<PathBuf>> {
    resolve_staged_path(path, staging, final_root, true)
}

fn staged_executable_if_exists(path: &Path, staging: &Path, final_root: &Path) -> Result<bool> {
    let Some(path) = staged_path_if_exists(path, staging, final_root)? else {
        return Ok(false);
    };
    Ok(path.is_file() && executable_metadata(&path)?)
}

fn executable_metadata(path: &Path) -> Result<bool> {
    Ok(fs::metadata(path)?.permissions().mode() & 0o111 != 0)
}

pub(super) fn staged_path(path: &Path, staging: &Path, final_root: &Path) -> Result<PathBuf> {
    resolve_staged_path(path, staging, final_root, false)?.ok_or_else(|| {
        anyhow!(
            "failed to inspect {}: no such file or directory",
            path.display()
        )
    })
}

fn resolve_staged_path(
    path: &Path,
    staging: &Path,
    final_root: &Path,
    allow_missing: bool,
) -> Result<Option<PathBuf>> {
    let mut current = path.to_owned();
    let mut links = 0;
    'resolve: loop {
        let mut resolved = PathBuf::new();
        let mut components = current.components();
        while let Some(component) = components.next() {
            match component {
                Component::Prefix(_) | Component::RootDir | Component::CurDir => {
                    resolved.push(component.as_os_str());
                    continue;
                }
                Component::ParentDir => {
                    resolved.pop();
                    continue;
                }
                Component::Normal(part) => resolved.push(part),
            }
            let metadata = match fs::symlink_metadata(&resolved) {
                Ok(metadata) => metadata,
                Err(err)
                    if allow_missing && links == 0 && err.kind() == io::ErrorKind::NotFound =>
                {
                    return Ok(None);
                }
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("failed to inspect {}", resolved.display()));
                }
            };
            if !metadata.file_type().is_symlink() {
                continue;
            }
            if links == 40 {
                bail!("link chain is too deep: {}", path.display());
            }
            links += 1;
            let target = fs::read_link(&resolved)
                .with_context(|| format!("failed to read link {}", resolved.display()))?;
            let target = if target.is_absolute() {
                let target = lexical_normalize(&target);
                if let Ok(relative) = target.strip_prefix(final_root) {
                    staging.join(relative)
                } else {
                    target
                }
            } else {
                resolved.parent().unwrap_or(Path::new(".")).join(target)
            };
            current = lexical_normalize(&target.join(components.as_path()));
            continue 'resolve;
        }
        return Ok(Some(resolved));
    }
}
