use std::fs;
use std::path::Path;

use serde::Deserialize;

pub const ALIASES: [&str; 2] = ["GE-Proton-Latest", "Proton-CachyOS-Latest"];

pub fn is_alias(name: &str) -> bool {
    ALIASES.contains(&name)
}

#[derive(Deserialize)]
struct Receipt {
    provider: String,
    version: String,
    architecture: String,
    variant: String,
    stable: bool,
}

pub(crate) fn version(root: &Path, name: &str, alias: &str) -> Option<Vec<u64>> {
    let provider = match alias {
        "GE-Proton-Latest" => "ge-proton",
        "Proton-CachyOS-Latest" => "proton-cachyos",
        _ => return None,
    };
    let receipt_path = root.join(name).join(".wineo-install.json");
    let receipt = match fs::read(&receipt_path) {
        Ok(bytes) => Some(serde_json::from_slice::<Receipt>(&bytes).ok()?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return None,
    };
    let version = if let Some(receipt) = &receipt {
        if receipt.provider != provider
            || !receipt.stable
            || receipt.architecture != "x86_64"
            || receipt.variant
                != if provider == "ge-proton" {
                    "baseline"
                } else {
                    "slr"
                }
        {
            return None;
        }
        receipt.version.as_str()
    } else {
        name
    };
    parse_version(version, provider)
}

pub(crate) fn complete_proton(root: &Path) -> bool {
    ["files/bin", "dist/bin"].iter().any(|bin| {
        ["wine", "wineserver"].iter().all(|exe| {
            let path = root.join(bin).join(exe);
            path.is_file() && crate::runner::is_executable(&path)
        })
    })
}

fn parse_version(name: &str, provider: &str) -> Option<Vec<u64>> {
    let name = name
        .strip_suffix("-x86_64")
        .or_else(|| name.strip_suffix("-x86-64"))
        .unwrap_or(name);
    let parts: Vec<&str> = if provider == "ge-proton" {
        name.strip_prefix("GE-Proton")?.split('-').collect()
    } else {
        let version = name
            .strip_prefix("proton-cachyos-")
            .or_else(|| name.strip_prefix("cachyos-"))?
            .strip_suffix("-slr")?;
        version
            .strip_suffix("-proton")
            .unwrap_or(version)
            .split(['.', '-'])
            .collect()
    };
    let expected = if provider == "ge-proton" { 2 } else { 3 };
    if parts.len() != expected
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    parts.into_iter().map(|part| part.parse().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_and_supported_variants_are_unambiguous() {
        assert_eq!(
            parse_version("GE-Proton11-7-x86_64", "ge-proton"),
            Some(vec![11, 7])
        );
        assert_eq!(
            parse_version("GE-Proton9-20", "ge-proton"),
            Some(vec![9, 20])
        );
        assert_eq!(
            parse_version("proton-cachyos-11.0-20260703-slr-x86_64", "proton-cachyos"),
            Some(vec![11, 0, 20260703])
        );
        for invalid in [
            "GE-Proton11-7-aarch64",
            "GE-Proton11-7-rc1",
            "GE-Proton-Latest",
            "GE-Proton11-7-custom",
        ] {
            assert!(parse_version(invalid, "ge-proton").is_none());
        }
        for invalid in [
            "proton-cachyos-11.0-20260703-slr-x86_64_v3",
            "proton-cachyos-11.0-20260703-base",
            "GE-Proton11-7",
        ] {
            assert!(parse_version(invalid, "proton-cachyos").is_none());
        }
    }
}
