//! Choosing a Galaxy build and what of it to install: the latest generation
//! 2 build on the branch followed, the depots of the base game and owned
//! DLC in the language asked for and the prefix's arch, and their files as
//! wineo writes them.

use std::collections::{BTreeMap, HashMap};

use wineo_gog::{Build, Builds, Depot, DepotFile, DepotItem, DepotManifest, Repository};

use super::record::{ChunkEntry, DirEntry, FileEntry, LinkEntry, Manifest, SetupInfo};
use crate::config::Arch;
use crate::gog::package;

/// The MD5 of nothing, an empty file's.
const EMPTY_MD5: &str = "d41d8cd98f00b204e9800998ecf8427e";

/// The latest build on `branch` (None for the default one): the first
/// generation 2 build GOG lists there, since it lists each branch newest
/// first. Generation 1 builds aren't supported.
pub(super) fn latest<'a>(builds: &'a Builds, branch: Option<&str>) -> Option<&'a Build> {
    builds
        .items
        .iter()
        .find(|build| build.branch.as_deref() == branch && build.generation == Some(2))
}

/// What `osBitness` calls a prefix's arch.
pub(super) fn bits(arch: Arch) -> &'static str {
    match arch {
        Arch::Win64 => "64",
        Arch::Win32 => "32",
    }
}

/// The language a product installs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Language {
    /// A depot's language code, like `en-US`, or the tag asked for when the
    /// product's depots are for every language.
    pub(super) code: String,
    /// Whether it fell back to English.
    pub(super) english: bool,
}

/// The language `product`'s depots are in, from `wanted`: the same code,
/// else the same language in another region, else English. None when the
/// product has no depot at all, or language-specific ones in none of those.
pub(super) fn pick_language(depots: &[Depot], product: u64, wanted: &str) -> Option<Language> {
    let own: Vec<&Depot> = depots
        .iter()
        .filter(|depot| depot.product_id == product)
        .collect();
    if own.is_empty() {
        return None;
    }
    let mut codes: Vec<&str> = Vec::new();
    for depot in &own {
        for code in &depot.languages {
            if code != "*" && !codes.iter().any(|seen| seen.eq_ignore_ascii_case(code)) {
                codes.push(code);
            }
        }
    }
    if codes.is_empty() {
        return Some(Language {
            code: wanted.to_owned(),
            english: false,
        });
    }
    let found = |wanted: &str| {
        codes
            .iter()
            .find(|code| code.eq_ignore_ascii_case(wanted))
            .or_else(|| codes.iter().find(|code| primary(code) == primary(wanted)))
            .map(|code| (*code).to_owned())
    };
    if let Some(code) = found(wanted) {
        return Some(Language {
            code,
            english: false,
        });
    }
    let english = found("en-US")?;
    Some(Language {
        english: primary(&english) != primary(wanted),
        code: english,
    })
}

fn primary(code: &str) -> String {
    code.split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Whether a depot is installed for `product` in `language` on a prefix of
/// `bits`.
pub(super) fn keeps(depot: &Depot, product: u64, language: &str, bits: &str) -> bool {
    depot.product_id == product && depot.has_language(language) && depot.fits_bitness(bits)
}

/// Who an entry comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Owner {
    Product(u64),
    /// A redistributable that is part of the game, like DOSBox.
    Dependency(String),
}

impl Owner {
    fn split(&self) -> (Option<u64>, Option<String>) {
        match self {
            Self::Product(id) => (Some(*id), None),
            Self::Dependency(id) => (None, Some(id.clone())),
        }
    }
}

/// One entry being assembled, under the spelling its path ends up with.
enum Assembled {
    File(FileEntry),
    Directory(DirEntry),
    Link(LinkEntry),
}

impl Assembled {
    fn path(&self) -> &str {
        match self {
            Self::File(file) => &file.path,
            Self::Directory(dir) => &dir.path,
            Self::Link(link) => &link.path,
        }
    }
}

/// Paths matched case-insensitively, each folder keeping the spelling it
/// was first seen with, since Windows games mix cases freely.
#[derive(Default)]
struct Spellings {
    /// Lowercased path to its spelling.
    known: HashMap<String, String>,
}

impl Spellings {
    fn spell(&mut self, parts: &[&str]) -> String {
        let mut spelled = String::new();
        let mut lower = String::new();
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                lower.push('/');
            }
            lower.push_str(&part.to_lowercase());
            let name = self
                .known
                .entry(lower.clone())
                .or_insert_with(|| {
                    if spelled.is_empty() {
                        (*part).to_owned()
                    } else {
                        format!("{spelled}/{part}")
                    }
                })
                .clone();
            spelled = name;
        }
        spelled
    }
}

/// A failure to make one tree of a build's files.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("{path} can't go inside {parent}, which the build ships as a file or link.")]
    Layers { path: String, parent: String },
    #[error("The build's link {path} points outside the game folder.")]
    LinkOutside { path: String },
}

/// The manifest of a build's files: each source's items in order, a later
/// one winning a path an earlier one has (matched case-insensitively), with
/// every folder a path implies, and support files apart. Links must stay
/// inside the folder.
pub(super) fn assemble(
    build: &str,
    sources: &[(Owner, &DepotManifest)],
    setup: SetupInfo,
) -> Result<Manifest, LayoutError> {
    let mut spellings = Spellings::default();
    let mut entries: BTreeMap<String, Assembled> = BTreeMap::new();
    let mut support: BTreeMap<(u64, String), FileEntry> = BTreeMap::new();
    let mut support_spellings: HashMap<u64, Spellings> = HashMap::new();
    for (owner, manifest) in sources {
        let (product, dependency) = owner.split();
        for item in &manifest.items {
            match item {
                DepotItem::File(file) => {
                    let parts = file.parts();
                    let entry = |path: String| file_entry(file, path, owner);
                    match (owner, file.is_support()) {
                        (Owner::Product(id), true) => {
                            let path = support_spellings.entry(*id).or_default().spell(&parts);
                            support.insert((*id, path.to_lowercase()), entry(path));
                        }
                        _ => {
                            let path = spellings.spell(&parts);
                            entries.insert(path.to_lowercase(), Assembled::File(entry(path)));
                        }
                    }
                }
                DepotItem::Directory(dir) => {
                    let path = spellings.spell(&dir.parts());
                    let key = path.to_lowercase();
                    // A folder never replaces a file or link a later source
                    // ships there; it's only implied by them.
                    entries.entry(key).or_insert(Assembled::Directory(DirEntry {
                        path,
                        product,
                        dependency: dependency.clone(),
                    }));
                }
                DepotItem::Link(link) => {
                    let path = spellings.spell(&link.parts());
                    entries.insert(
                        path.to_lowercase(),
                        Assembled::Link(LinkEntry {
                            target: link.target.replace('\\', "/"),
                            path,
                            product,
                            dependency: dependency.clone(),
                        }),
                    );
                }
            }
        }
    }

    // Every folder a path implies is an entry of its own, so it is tracked
    // like the rest. One that a file or link stands at can't hold anything.
    let mut implied = Vec::new();
    for entry in entries.values() {
        let path = entry.path();
        let mut parent = path;
        while let Some((up, _)) = parent.rsplit_once('/') {
            parent = up;
            match entries.get(&parent.to_lowercase()) {
                Some(Assembled::Directory(_)) => {}
                Some(_) => {
                    return Err(LayoutError::Layers {
                        path: path.to_owned(),
                        parent: parent.to_owned(),
                    });
                }
                None => {
                    let (product, dependency) = match entry {
                        Assembled::File(file) => (file.product, file.dependency.clone()),
                        Assembled::Directory(dir) => (dir.product, dir.dependency.clone()),
                        Assembled::Link(link) => (link.product, link.dependency.clone()),
                    };
                    implied.push((
                        parent.to_lowercase(),
                        DirEntry {
                            path: parent.to_owned(),
                            product,
                            dependency,
                        },
                    ));
                }
            }
        }
    }
    for (key, dir) in implied {
        entries.entry(key).or_insert(Assembled::Directory(dir));
    }

    let mut out = Manifest {
        build: build.to_owned(),
        setup,
        ..Manifest::default()
    };
    for entry in entries.into_values() {
        match entry {
            Assembled::File(file) => out.files.push(file),
            Assembled::Directory(dir) => out.directories.push(dir),
            Assembled::Link(link) => out.links.push(link),
        }
    }
    out.support = support.into_values().collect();
    check_links(&out.links)?;
    sort(&mut out);
    Ok(out)
}

/// Sorts every list by path, which puts each folder before what is in it.
pub(super) fn sort(manifest: &mut Manifest) {
    manifest.files.sort_by(|a, b| a.path.cmp(&b.path));
    manifest.directories.sort_by(|a, b| a.path.cmp(&b.path));
    manifest.links.sort_by(|a, b| a.path.cmp(&b.path));
    manifest
        .support
        .sort_by(|a, b| (a.product, &a.path).cmp(&(b.product, &b.path)));
}

/// Links must be relative and resolve inside the game, following the
/// build's other links.
fn check_links(links: &[LinkEntry]) -> Result<(), LayoutError> {
    for link in links {
        if link.target.starts_with('/') || link.target.contains([':', '\0']) {
            return Err(LayoutError::LinkOutside {
                path: link.path.clone(),
            });
        }
    }
    let map: BTreeMap<&str, &str> = links
        .iter()
        .map(|link| (link.path.as_str(), link.target.as_str()))
        .collect();
    package::check_link_targets(&map).map_err(|error| {
        // The resolver names the link it was checking.
        let path = links
            .iter()
            .find(|link| error.to_string().contains(&link.path))
            .map_or_else(String::new, |link| link.path.clone());
        LayoutError::LinkOutside { path }
    })
}

fn file_entry(file: &DepotFile, path: String, owner: &Owner) -> FileEntry {
    let (product, dependency) = owner.split();
    let md5 = file.md5.clone().or_else(|| match file.chunks.as_slice() {
        [] => Some(EMPTY_MD5.to_owned()),
        [only] => Some(only.md5.clone()),
        _ => None,
    });
    FileEntry {
        path,
        product,
        dependency,
        size: file.size(),
        md5,
        sha256: file.sha256.clone(),
        chunks: file
            .chunks
            .iter()
            .map(|chunk| ChunkEntry {
                md5: chunk.md5.clone(),
                size: chunk.size,
                compressed_md5: chunk.compressed_md5.clone(),
                compressed_size: chunk.compressed_size,
            })
            .collect(),
    }
}

/// The file as the client downloads it.
pub(super) fn depot_file(file: &FileEntry) -> DepotFile {
    DepotFile {
        path: file.path.clone(),
        chunks: file
            .chunks
            .iter()
            .map(|chunk| wineo_gog::Chunk {
                md5: chunk.md5.clone(),
                size: chunk.size,
                compressed_md5: chunk.compressed_md5.clone(),
                compressed_size: chunk.compressed_size,
            })
            .collect(),
        md5: file.md5.clone(),
        sha256: file.sha256.clone(),
        flags: Vec::new(),
    }
}

/// The products of a repository that are kept, with each one's setup
/// program, in the repository's order.
pub(super) fn product_setups(
    repository: &Repository,
    kept: &[u64],
) -> Vec<super::record::ProductSetup> {
    repository
        .products
        .iter()
        .filter(|product| kept.contains(&product.product_id))
        .map(|product| super::record::ProductSetup {
            product: product.product_id,
            temp_executable: product
                .temp_executable
                .as_ref()
                .map(|path| path.replace('\\', "/")),
            temp_arguments: product.temp_arguments.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn depot(product: u64, languages: &[&str], bits: &[&str]) -> Depot {
        Depot {
            product_id: product,
            languages: languages.iter().map(|code| (*code).to_owned()).collect(),
            os_bitness: bits.iter().map(|bits| (*bits).to_owned()).collect(),
            manifest: "0".repeat(32),
            size: 0,
            compressed_size: 0,
            is_gog_depot: false,
        }
    }

    fn build(id: &str, branch: Option<&str>, generation: u64) -> Build {
        Build {
            build_id: id.to_owned(),
            version_name: None,
            branch: branch.map(str::to_owned),
            date_published: None,
            generation: Some(generation),
            link: None,
        }
    }

    #[test]
    fn the_latest_build_is_the_first_generation_2_one_on_the_branch() {
        let builds = Builds {
            items: vec![
                build("3", None, 1),
                build("2", None, 2),
                build("9", Some("beta"), 2),
                build("1", None, 2),
            ],
            has_private_branches: false,
        };
        assert_eq!(latest(&builds, None).unwrap().build_id, "2");
        assert_eq!(latest(&builds, Some("beta")).unwrap().build_id, "9");
        let old = Builds {
            items: vec![build("3", None, 1)],
            has_private_branches: false,
        };
        assert!(latest(&old, None).is_none());
    }

    #[test]
    fn languages_match_exactly_then_by_language_then_fall_back_to_english() {
        let depots = [
            depot(1, &["*"], &[]),
            depot(1, &["en-US"], &[]),
            depot(1, &["fr-FR", "de-DE"], &[]),
        ];
        let pick = |wanted| pick_language(&depots, 1, wanted).unwrap();
        assert_eq!(pick("fr-FR").code, "fr-FR");
        assert_eq!(pick("FR-ca").code, "fr-FR");
        assert!(!pick("fr-CA").english);
        assert_eq!(pick("ja-JP").code, "en-US");
        assert!(pick("ja-JP").english);
        assert!(!pick("en-GB").english);
        // Only depots for every language: anything goes.
        let neutral = [depot(1, &["*"], &[])];
        assert_eq!(pick_language(&neutral, 1, "pl-PL").unwrap().code, "pl-PL");
        assert_eq!(pick_language(&neutral, 2, "pl-PL"), None);
        let no_english = [depot(1, &["de-DE"], &[])];
        assert_eq!(pick_language(&no_english, 1, "fr-FR"), None);
    }

    #[test]
    fn depots_are_kept_by_product_language_and_arch() {
        let both = depot(1, &["*"], &[]);
        let only32 = depot(1, &["*"], &["32"]);
        let only64 = depot(1, &["en-US"], &["64"]);
        assert!(keeps(&both, 1, "en-US", "32"));
        assert!(keeps(&only32, 1, "en-US", "32"));
        assert!(!keeps(&only32, 1, "en-US", "64"));
        assert!(keeps(&only64, 1, "en-US", "64"));
        assert!(!keeps(&only64, 1, "fr-FR", "64"));
        assert!(!keeps(&both, 2, "en-US", "64"));
    }

    fn file(path: &str, flags: &[&str]) -> DepotItem {
        DepotItem::File(DepotFile {
            path: path.to_owned(),
            chunks: vec![wineo_gog::Chunk {
                md5: "1".repeat(32),
                size: 3,
                compressed_md5: "2".repeat(32),
                compressed_size: 5,
            }],
            md5: None,
            sha256: None,
            flags: flags.iter().map(|flag| (*flag).to_owned()).collect(),
        })
    }

    #[test]
    fn assembling_merges_cases_implies_folders_and_keeps_support_apart() {
        let base = DepotManifest {
            items: vec![
                file("Data\\Core.dat", &[]),
                file("bin\\game.exe", &["executable"]),
                file("setup\\setup.exe", &["support"]),
            ],
        };
        let dlc = DepotManifest {
            items: vec![file("DATA\\dlc.dat", &[]), file("data\\core.DAT", &[])],
        };
        let manifest = assemble(
            "7",
            &[(Owner::Product(1), &base), (Owner::Product(2), &dlc)],
            SetupInfo::default(),
        )
        .unwrap();
        let paths: Vec<(&str, Option<u64>)> = manifest
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.product))
            .collect();
        // The DLC's copy wins, under the spelling seen first.
        assert_eq!(
            paths,
            [
                ("Data/Core.dat", Some(2)),
                ("Data/dlc.dat", Some(2)),
                ("bin/game.exe", Some(1)),
            ]
        );
        let dirs: Vec<&str> = manifest
            .directories
            .iter()
            .map(|dir| dir.path.as_str())
            .collect();
        assert_eq!(dirs, ["Data", "bin"]);
        assert_eq!(manifest.support.len(), 1);
        assert_eq!(manifest.support[0].path, "setup/setup.exe");
        assert_eq!(manifest.support[0].product, Some(1));
        // One chunk: its MD5 is the file's.
        assert_eq!(
            manifest.files[0].md5.as_deref(),
            Some("1".repeat(32).as_str())
        );
    }

    #[test]
    fn links_leaving_the_folder_and_files_inside_files_are_refused() {
        let link = |target: &str| DepotManifest {
            items: vec![DepotItem::Link(wineo_gog::DepotLink {
                path: "a\\link".to_owned(),
                target: target.to_owned(),
            })],
        };
        for target in ["../../etc", "/etc/passwd", "C:\\Windows", "..\\..\\x"] {
            let error = assemble(
                "1",
                &[(Owner::Product(1), &link(target))],
                SetupInfo::default(),
            )
            .unwrap_err();
            assert_eq!(
                error,
                LayoutError::LinkOutside {
                    path: "a/link".to_owned()
                },
                "{target}"
            );
        }
        assert!(
            assemble(
                "1",
                &[(Owner::Product(1), &link("..\\b"))],
                SetupInfo::default()
            )
            .is_ok()
        );
        let nested = DepotManifest {
            items: vec![file("a", &[]), file("A\\b", &[])],
        };
        assert!(matches!(
            assemble("1", &[(Owner::Product(1), &nested)], SetupInfo::default()),
            Err(LayoutError::Layers { .. })
        ));
    }
}
