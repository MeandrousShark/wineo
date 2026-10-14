//! A stand-in for GOG's servers in tests, like the core and CLI tests use:
//! plain HTTP on localhost, answering each request line through a handler.

use crate::test_script::write_script;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wineo_core::key_store::Keyring;

pub(crate) use wineo_gog::fake::Scripted as Server;

pub(crate) fn reply(status: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// A token endpoint answer.
pub(crate) fn tokens(refresh: &str, user_id: u64) -> Vec<u8> {
    let body = serde_json::json!({
        "access_token": "access-token-value",
        "refresh_token": refresh,
        "expires_in": 3600,
        "user_id": user_id.to_string(),
    });
    reply(200, &body.to_string())
}

/// Answers GOG's token endpoint and `/products/<id>` from `products`, and
/// serves their installers: `/downlink/<file>` resolves a file and
/// `/files/<file>` sends `FILE_SIZE` bytes.
pub(crate) fn product_server(products: Vec<serde_json::Value>) -> Server {
    product_server_slow(products, Duration::ZERO)
}

/// `product_server`, taking `delay` to answer each `/products` request, like
/// a slow GOG.
pub(crate) fn product_server_slow(products: Vec<serde_json::Value>, delay: Duration) -> Server {
    product_server_with(products, delay, usize::MAX, Duration::ZERO)
}

/// `product_server`, sending every answer 64 KiB at a time a few
/// milliseconds apart, so a download is still coming in when it's canceled.
pub(crate) fn product_server_streaming(products: Vec<serde_json::Value>) -> Server {
    product_server_with(
        products,
        Duration::ZERO,
        64 * 1024,
        Duration::from_millis(5),
    )
}

fn product_server_with(
    products: Vec<serde_json::Value>,
    delay: Duration,
    chunk: usize,
    pause: Duration,
) -> Server {
    let base = Arc::new(std::sync::OnceLock::<String>::new());
    let seen = Arc::clone(&base);
    let server = Server::start_paced(
        move |line| {
            let base = seen.get().cloned().unwrap_or_default();
            let path = line.split(' ').nth(1).unwrap_or_default();
            let path = path.split('?').next().unwrap_or_default();
            if path == "/token" {
                return tokens("REFRESH", USER);
            }
            let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
            match parts.as_slice() {
                ["products", id] => {
                    std::thread::sleep(delay);
                    products
                        .iter()
                        .find(|product| id.parse::<u64>().ok() == product["id"].as_u64())
                        .map_or_else(
                            || reply(404, "{}"),
                            |body| reply(200, &body.to_string().replace("{BASE}", &base)),
                        )
                }
                ["downlink", name] => reply(
                    200,
                    &serde_json::json!({
                        "downlink": format!("{base}/files/{name}"),
                        "checksum": null,
                    })
                    .to_string(),
                ),
                ["files", _] => {
                    let mut answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {FILE_SIZE}\r\nConnection: close\r\n\r\n"
                )
                .into_bytes();
                    answer.resize(answer.len() + FILE_SIZE as usize, 0);
                    answer
                }
                _ => reply(404, "{}"),
            }
        },
        chunk,
        pause,
    );
    let _ = base.set(server.base.clone());
    server
}

/// The size of every installer the fake GOG serves.
pub(crate) const FILE_SIZE: u64 = 7340032;

pub(crate) const USER: u64 = 1001;

/// GOG's details for a native game with one Linux installer.
pub(crate) fn native_product(id: u64, title: &str, version: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "downloads": {"installers": [{
            "id": format!("installer_linux_en_{id}"),
            "os": "linux",
            "language": "en",
            "version": version,
            "total_size": FILE_SIZE,
            "files": [{
                "id": "f",
                "size": FILE_SIZE,
                "downlink": format!("{{BASE}}/downlink/game_{id}.sh"),
            }],
        }]},
        "expanded_dlcs": [],
    })
}

/// `native_product` with each of `dlc` (id and title) as an expanded DLC
/// offering the same version.
pub(crate) fn native_product_with_dlc(
    id: u64,
    title: &str,
    version: &str,
    dlc: &[(u64, &str)],
) -> serde_json::Value {
    let mut product = native_product(id, title, version);
    product["expanded_dlcs"] = dlc
        .iter()
        .map(|(id, title)| native_product(*id, title, version))
        .collect();
    product
}

/// Saves a login the fake server's token endpoint renews.
pub(crate) fn save_login(paths: &wineo_core::paths::Paths, keyring: &dyn Keyring) {
    let secret = format!(r#"{{"refresh_token":"R","user_id":"{USER}"}}"#);
    wineo_core::key_store::save_key(&wineo_core::gog::account::KEY_SPEC, paths, keyring, &secret)
        .unwrap();
}

/// A native GOG game folder under `root` owned by environment `name`, which
/// launches its `start.sh`, at installed `version`, with a journal when
/// `interrupted`.
pub(crate) fn native_game(
    paths: &wineo_core::paths::Paths,
    root: &std::path::Path,
    name: &str,
    gog_id: u64,
    version: &str,
    interrupted: bool,
) -> std::path::PathBuf {
    native_game_with_dlc(paths, root, name, gog_id, version, &[], interrupted)
}

/// `native_game` with each of `dlc` installed at the same version.
pub(crate) fn native_game_with_dlc(
    paths: &wineo_core::paths::Paths,
    root: &std::path::Path,
    name: &str,
    gog_id: u64,
    version: &str,
    dlc: &[u64],
    interrupted: bool,
) -> std::path::PathBuf {
    use wineo_core::gog::folder::{
        Committed, FolderLock, FolderState, Journal, JournalKind, Platform, Product, ProductStatus,
        Target, Track,
    };
    let folder = std::fs::canonicalize(root)
        .unwrap()
        .join("Games")
        .join(name);
    std::fs::create_dir_all(&folder).unwrap();
    let start = folder.join("start.sh");
    write_script(&start, "#!/bin/sh\n");
    native_environment(paths, name, &start, Some(gog_id));
    let products: Vec<Product> = std::iter::once(gog_id)
        .chain(dlc.iter().copied())
        .map(|id| Product {
            reported_language: None,
            id,
            version: Some(version.to_owned()),
            build: None,
            language: None,
            status: ProductStatus::Installed,
        })
        .collect();
    let state = FolderState {
        committed: Some(Committed {
            source: None,
            gog_id,
            platform: Platform::Linux,
            language: "en-US".to_owned(),
            track: Track::Installer,
            products: products.clone(),
            files: Vec::new(),
        }),
        journal: interrupted.then(|| Journal {
            operation: "op".to_owned(),
            kind: JournalKind::Update,
            target: Target {
                gog_id,
                platform: Platform::Linux,
                language: "en-US".to_owned(),
                track: Track::Installer,
                products,
            },
            packages: Vec::new(),
            attempted: Vec::new(),
        }),
        ..FolderState::new(folder.clone(), Some(name.to_owned()))
    };
    FolderLock::exclusive(paths, &folder, &|| false)
        .unwrap()
        .write(&state)
        .unwrap();
    folder
}

/// A native environment that runs `exe`, tracking `gog_id` when given.
pub(crate) fn native_environment(
    paths: &wineo_core::paths::Paths,
    name: &str,
    exe: &std::path::Path,
    gog_id: Option<u64>,
) {
    let config = paths.environment_config(name);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let gog = gog_id.map_or_else(String::new, |id| format!("gog_id = \"{id}\"\n"));
    std::fs::write(
        config,
        format!(
            "kind = \"native\"\ntags = [\"game\"]\n{gog}\n[targets.game]\nexe = \"{}\"\ndefault = true\n",
            exe.display()
        ),
    )
    .unwrap();
}

/// What `library_server` serves: the owned list, the owned ids, product
/// details with `{BASE}` standing for the server, and installer files by
/// name.
#[derive(Default)]
pub(crate) struct Library {
    pub(crate) owned: Vec<serde_json::Value>,
    pub(crate) owned_ids: Vec<u64>,
    pub(crate) products: Vec<serde_json::Value>,
    pub(crate) files: std::collections::HashMap<String, Vec<u8>>,
}

/// A fake GOG with a whole account: its owned games and ids, product
/// details, and the installers they name, kept in `library` so a test can
/// change what's offered while the server runs.
pub(crate) fn library_server(library: Arc<Mutex<Library>>) -> Server {
    let base = Arc::new(std::sync::OnceLock::<String>::new());
    let seen = Arc::clone(&base);
    let server = Server::start(move |line| {
        let base = seen.get().cloned().unwrap_or_default();
        let path = line.split(' ').nth(1).unwrap_or_default();
        let path = path.split('?').next().unwrap_or_default();
        if path == "/token" {
            return tokens("REFRESH", USER);
        }
        let library = library.lock().unwrap();
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        match parts.as_slice() {
            ["account", "getFilteredProducts"] => reply(
                200,
                &serde_json::json!({"page": 1, "totalPages": 1, "products": library.owned})
                    .to_string(),
            ),
            ["user", "data", "games"] => reply(
                200,
                &serde_json::json!({"owned": library.owned_ids}).to_string(),
            ),
            ["products", id] => library
                .products
                .iter()
                .find(|product| id.parse::<u64>().ok() == product["id"].as_u64())
                .map_or_else(
                    || reply(404, "{}"),
                    |body| reply(200, &body.to_string().replace("{BASE}", &base)),
                ),
            ["downlink", name] => reply(
                200,
                &serde_json::json!({
                    "downlink": format!("{base}/files/{name}"),
                    "checksum": null,
                })
                .to_string(),
            ),
            ["files", name] => match library.files.get(*name) {
                Some(bytes) => {
                    let mut answer = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .into_bytes();
                    answer.extend_from_slice(bytes);
                    answer
                }
                None => reply(404, "{}"),
            },
            _ => reply(404, "{}"),
        }
    });
    let _ = base.set(server.base.clone());
    server
}

/// An owned game as GOG's list shows it.
pub(crate) fn owned_game(id: u64, title: &str, linux: bool, windows: bool) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "worksOn": {"Windows": windows, "Mac": false, "Linux": linux},
        "dlcCount": 0,
    })
}

/// A product's details with `installers` and each of `dlcs` expanded.
pub(crate) fn product(
    id: u64,
    title: &str,
    installers: Vec<serde_json::Value>,
    dlcs: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": title,
        "downloads": {"installers": installers, "patches": []},
        "dlcs": {"products": dlcs.iter().map(|dlc| serde_json::json!({"id": dlc["id"]})).collect::<Vec<_>>()},
        "expanded_dlcs": dlcs,
    })
}

impl Library {
    /// Offers a Linux installer for `id` whose one file is a package of
    /// `files` (path and content; `start.sh` is executable), and returns its
    /// entry for a product's details.
    pub(crate) fn linux_installer(
        &mut self,
        id: u64,
        version: &str,
        files: &[(&str, &str)],
    ) -> serde_json::Value {
        let name = format!("game_{id}_{}.sh", safe(version));
        let bytes = native_package(files);
        let size = bytes.len();
        self.files.insert(name.clone(), bytes);
        serde_json::json!({
            "id": format!("installer_linux_en_{id}"),
            "os": "linux",
            "language": "en",
            "language_full": "English",
            "version": version,
            "total_size": size,
            "files": [{"id": "en3installer0", "size": size, "downlink": format!("{{BASE}}/downlink/{name}")}],
        })
    }

    /// Offers a Windows installer for `id`: a setup program, `script`, which
    /// the fake `inno` runner sources with `$DIR` set, and a part beside it.
    pub(crate) fn windows_installer(
        &mut self,
        id: u64,
        version: &str,
        script: &str,
    ) -> serde_json::Value {
        let stem = format!("setup_{id}_{}", safe(version));
        let exe = format!("{stem}.exe");
        let part = format!("{stem}-1.bin");
        let script = format!("[ -f \"$(dirname \"$exe\")/{part}\" ] || exit 99\n{script}");
        let sizes = [script.len(), 4];
        self.files.insert(exe.clone(), script.into_bytes());
        self.files.insert(part.clone(), b"part".to_vec());
        serde_json::json!({
            "id": format!("installer_windows_en_{id}"),
            "os": "windows",
            "language": "en",
            "language_full": "English",
            "version": version,
            "total_size": sizes[0] + sizes[1],
            "files": [
                {"id": "en1installer0", "size": sizes[0], "downlink": format!("{{BASE}}/downlink/{exe}")},
                {"id": "en1installer1", "size": sizes[1], "downlink": format!("{{BASE}}/downlink/{part}")},
            ],
        })
    }

    /// Replaces `id`'s details.
    pub(crate) fn offer(&mut self, details: serde_json::Value) {
        let id = details["id"].as_u64();
        self.products.retain(|product| product["id"].as_u64() != id);
        self.products.push(details);
    }
}

fn safe(version: &str) -> String {
    version.replace(|char: char| !char.is_ascii_alphanumeric(), "_")
}

/// A native package like GOG's `.sh`: a shell header, a MojoSetup stand-in,
/// and a zip of the game files, `start.sh` executable.
pub(crate) fn native_package(files: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, content) in files {
        let mode = if path.ends_with(".sh") { 0o755 } else { 0o644 };
        zip.start_file(
            format!("data/noarch/{path}"),
            SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(mode),
        )
        .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    let zipped = zip.finish().unwrap().into_inner();
    let mut bytes = b"#!/bin/sh\n# Makeself stand-in\nexit 0\n".to_vec();
    bytes.extend(b"MOJOSETUP".repeat(50));
    bytes.extend(zipped);
    bytes
}

/// A runner `inno` whose wine sources a setup program as a shell script,
/// with `$DIR` set from its `/DIR=` argument, after writing its arguments to
/// `runs/<name>.args`, like the CLI tests' runner.
pub(crate) fn inno_runner(paths: &wineo_core::paths::Paths, runs: &std::path::Path) {
    std::fs::create_dir_all(runs).unwrap();
    let bin = wineo_core::config::GlobalConfig::default()
        .runner_root(paths)
        .unwrap()
        .join("inno/bin");
    std::fs::create_dir_all(&bin).unwrap();
    write_script(bin.join("wineserver"), "#!/bin/sh\nexit 0\n");
    write_script(
        bin.join("wine"),
        format!(
            r#"#!/bin/sh
RUNS='{runs}'
case "$1" in
wineboot) [ "$2" = --init ] && mkdir -p "$WINEPREFIX/drive_c" && echo 'WINE REGISTRY Version 2' > "$WINEPREFIX/system.reg"; exit 0 ;;
--version) echo wine-inno; exit 0 ;;
*.exe) ;;
*) exit 0 ;;
esac
exe="$1"
shift
name=$(basename "$exe")
printf '%s\n' "$exe" "$@" > "$RUNS/$name.args"
DIR=
for arg in "$@"; do
    case "$arg" in /DIR=Z:*) DIR=$(printf '%s' "${{arg#/DIR=Z:}}" | tr '\\' '/') ;; esac
done
. "$exe"
"#,
            runs = runs.display()
        ),
    );
}
