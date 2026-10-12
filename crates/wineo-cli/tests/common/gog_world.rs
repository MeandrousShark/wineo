//! A fixture's Heroic with GOG games installed, and wineo environments for
//! GOG games, to drive `import gog` and `update` against the fake GOG.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use wineo_core::gog::folder::{
    Committed, FolderLock, FolderState, Platform, Product, ProductStatus, Track,
};

use super::Fixture;

pub const RIMWORLD: u64 = 1094900565;
pub const DISCO: u64 = 1771589310;
pub const RIMWORLD_DLC: [u64; 2] = [1233017772, 1827857764];

impl Fixture {
    /// Heroic's config folder, under the fixture's XDG config.
    pub fn heroic(&self) -> PathBuf {
        self.root.path().join("config/heroic")
    }

    /// An empty Heroic that has written its config, with GOG's library cache.
    pub fn heroic_gog(&self) {
        let heroic = self.heroic();
        fs::create_dir_all(heroic.join("GamesConfig")).unwrap();
        fs::create_dir_all(heroic.join("gog_store")).unwrap();
        fs::create_dir_all(heroic.join("store_cache")).unwrap();
        fs::write(
            heroic.join("config.json"),
            r#"{"defaultSettings":{"wineVersion":{"type":"wine","name":"System","bin":"wine"}}}"#,
        )
        .unwrap();
        fs::write(
            heroic.join("store_cache/gog_library.json"),
            format!(
                r#"{{"games":[{{"runner":"gog","app_name":"{RIMWORLD}","title":"RimWorld","is_linux_native":true}},{{"runner":"gog","app_name":"{DISCO}","title":"Disco Elysium"}}],"__timestamp":1}}"#
            ),
        )
        .unwrap();
        self.write_installed(&[]);
    }

    fn installed_path(&self) -> PathBuf {
        self.heroic().join("gog_store/installed.json")
    }

    fn write_installed(&self, entries: &[Value]) {
        fs::write(
            self.installed_path(),
            json!({ "installed": entries }).to_string(),
        )
        .unwrap();
    }

    fn add_installed(&self, entry: Value) {
        let text = fs::read_to_string(self.installed_path()).unwrap();
        let mut root: Value = serde_json::from_str(&text).unwrap();
        root["installed"].as_array_mut().unwrap().push(entry);
        fs::write(self.installed_path(), root.to_string()).unwrap();
    }

    /// RimWorld, native, installed by Heroic in `Games/RimWorld`, with a mod
    /// folder it doesn't ship.
    pub fn heroic_rimworld(&self) -> PathBuf {
        let dir = self.root.path().join("Games/RimWorld");
        fs::create_dir_all(dir.join("mods-1.5/MyMod")).unwrap();
        fs::write(
            dir.join("gameinfo"),
            format!("RimWorld\n1.6.4871 rev588\n93132\nen-US\n{RIMWORLD}\n{RIMWORLD}\n1\n"),
        )
        .unwrap();
        let start = dir.join("start.sh");
        fs::write(&start, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&start, fs::Permissions::from_mode(0o755)).unwrap();
        self.add_installed(json!({
            "platform": "linux", "executable": "", "install_path": dir,
            "install_size": "5.03 GiB", "is_dlc": false, "version": "1.6.4871 rev588",
            "appName": RIMWORLD.to_string(), "installedDLCs": RIMWORLD_DLC,
            "language": "en", "versionEtag": "", "buildId": "", "pinnedVersion": false,
        }));
        dir
    }

    /// Disco Elysium, Windows, installed by Heroic from Galaxy in
    /// `Games/Disco`, with its own prefix, run by `runner`.
    pub fn heroic_disco(&self, runner: Value) -> PathBuf {
        let dir = self.root.path().join("Games/Disco");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Disco Elysium.exe"), "game").unwrap();
        fs::write(
            dir.join(format!("goggame-{DISCO}.info")),
            json!({
                "gameId": DISCO.to_string(),
                "rootGameId": DISCO.to_string(),
                "buildId": "59781086834418085",
                "playTasks": [{
                    "isPrimary": true, "type": "FileTask", "path": "Disco Elysium.exe",
                    "category": "game",
                }],
            })
            .to_string(),
        )
        .unwrap();
        let prefix = self.root.path().join("Prefixes/Disco");
        // A Proton prefix keeps the wine prefix in `pfx`.
        let wine_prefix = if runner["type"] == "proton" {
            prefix.join("pfx")
        } else {
            prefix.clone()
        };
        fs::create_dir_all(wine_prefix.join("drive_c")).unwrap();
        fs::write(
            wine_prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        fs::write(
            self.heroic().join(format!("GamesConfig/{DISCO}.json")),
            json!({ DISCO.to_string(): { "winePrefix": prefix, "wineVersion": runner } })
                .to_string(),
        )
        .unwrap();
        self.add_installed(json!({
            "platform": "windows", "executable": "", "install_path": dir,
            "install_size": "9.57 GiB", "is_dlc": false, "version": "WINkay16062026_memfix",
            "appName": DISCO.to_string(), "installedDLCs": [], "language": "en-US",
            "versionEtag": "", "buildId": "59781086834418085", "pinnedVersion": false,
        }));
        dir
    }

    /// A native environment `name` that owns `folder` as GOG game `gog_id`,
    /// installed at `version` with the given DLC (id and version).
    pub fn gog_environment(
        &self,
        name: &str,
        folder: &Path,
        gog_id: u64,
        version: &str,
        dlc: &[(u64, Option<&str>)],
    ) {
        let config = self.config_dir().join(format!("environments/{name}.toml"));
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(
            config,
            format!(
                "kind = \"native\"\ngog_id = \"{gog_id}\"\n\n[targets.game]\nexe = \"{}\"\n",
                folder.join("start.sh").display()
            ),
        )
        .unwrap();
        let products = std::iter::once(Product {
            id: gog_id,
            version: Some(version.to_owned()),
            build: None,
            status: ProductStatus::Installed,
        })
        .chain(dlc.iter().map(|(id, version)| Product {
            id: *id,
            version: version.map(str::to_owned),
            build: None,
            status: ProductStatus::Installed,
        }))
        .collect();
        let state = FolderState {
            device: Some(fs::metadata(folder).unwrap().dev()),
            committed: Some(Committed {
                gog_id,
                platform: Platform::Linux,
                language: "en".to_owned(),
                track: Track::Installer,
                products,
                files: Vec::new(),
            }),
            ..FolderState::new(fs::canonicalize(folder).unwrap(), Some(name.to_owned()))
        };
        FolderLock::exclusive(&self.paths(), &state.folder, &|| false)
            .unwrap()
            .write(&state)
            .unwrap();
    }
}
