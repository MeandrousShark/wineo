//! A local HTTP server standing in for GitHub's release catalog and its
//! download links. Debug builds of wineo send every catalog request to the
//! server named by `WINEO_TEST_RELEASES_API` and may follow plain HTTP to it
//! and nowhere else, so the catalog and the archives it names live here.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Where the fixture's wineo looks unless a test names a server: a closed
/// port, so a command that would ask GitHub fails instead of reaching it.
pub const DEAD: &str = "http://127.0.0.1:9";

#[derive(Clone, Copy)]
pub enum Repo {
    GeProton,
    ProtonCachyos,
    Dxvk,
    Vkd3d,
}

impl Repo {
    /// The path of the repo's release list, which the catalog is asked for.
    pub fn releases_path(self) -> &'static str {
        match self {
            Self::GeProton => "/repos/GloriousEggroll/proton-ge-custom/releases",
            Self::ProtonCachyos => "/repos/CachyOS/proton-cachyos/releases",
            Self::Dxvk => "/repos/doitsujin/dxvk/releases",
            Self::Vkd3d => "/repos/HansKristian-Work/vkd3d-proton/releases",
        }
    }
}

#[derive(Clone)]
enum Route {
    Body(Vec<u8>),
    Status(u16),
    /// Announces `total` bytes and sends them a kilobyte at a time, so a
    /// download is still running when the test interrupts it.
    Drip(usize),
    /// Announces `total` bytes, sends a kilobyte, then hangs up once
    /// `release_stalls` is called, so the download fails on its own.
    Stall(usize),
}

pub struct ReleasesApi {
    pub base: String,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    requests: Arc<Mutex<Vec<String>>>,
    release: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ReleasesApi {
    pub fn start() -> Self {
        // A release build ignores the variable and would ask GitHub itself.
        if !cfg!(debug_assertions) {
            panic!("the fake release catalog needs a debug build");
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes = Arc::new(Mutex::new(HashMap::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (routes, requests, release, stop) = (
                routes.clone(),
                requests.clone(),
                release.clone(),
                stop.clone(),
            );
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let Ok((stream, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    };
                    let (routes, requests, release, stop) = (
                        routes.clone(),
                        requests.clone(),
                        release.clone(),
                        stop.clone(),
                    );
                    thread::spawn(move || serve(stream, &routes, &requests, &release, &stop));
                }
            })
        };
        Self {
            base,
            routes,
            requests,
            release,
            stop,
            thread: Some(thread),
        }
    }

    /// Lists `tag` as the repo's newest release, shipping `asset`, and serves
    /// the asset's bytes with the digest and size a real release carries.
    pub fn publish(&self, repo: Repo, tag: &str, asset: &str, body: Vec<u8>) {
        let digest = format!("sha256:{:x}", Sha256::digest(&body));
        self.list(repo, tag, asset, body.len() as u64, Some(&digest));
        self.route(&format!("/assets/{asset}"), Route::Body(body));
    }

    /// Lists a release without serving its archive, for a check that never
    /// downloads. `size` is what the catalog says, which no body has to match.
    pub fn announce(&self, repo: Repo, tag: &str, asset: &str, size: u64) {
        let digest = format!("sha256:{}", "0".repeat(64));
        self.list(repo, tag, asset, size, Some(&digest));
    }

    /// Lists a release whose archive arrives slowly and never finishes.
    pub fn drip(&self, repo: Repo, tag: &str, asset: &str, size: usize) {
        self.announce(repo, tag, asset, size as u64);
        self.route(&format!("/assets/{asset}"), Route::Drip(size));
    }

    /// Lists a release whose archive starts and then stalls until
    /// `release_stalls`, which hangs up on it.
    pub fn stall(&self, repo: Repo, tag: &str, asset: &str, size: usize) {
        self.announce(repo, tag, asset, size as u64);
        self.route(&format!("/assets/{asset}"), Route::Stall(size));
    }

    /// Hangs up on every stalled download.
    pub fn release_stalls(&self) {
        self.release.store(true, Ordering::Relaxed);
    }

    /// Answers `path` with an empty reply of this status.
    pub fn status(&self, path: &str, code: u16) {
        self.route(path, Route::Status(code));
    }

    /// The archive's own address answers with this status.
    pub fn asset_status(&self, asset: &str, code: u16) {
        self.status(&format!("/assets/{asset}"), code);
    }

    /// Serves `tag` on its own, as `--version TAG` asks for it, with the
    /// archive. A release that is listed too is the caller's to publish.
    pub fn publish_tag(&self, repo: Repo, tag: &str, asset: &str, body: Vec<u8>) {
        let digest = format!("sha256:{:x}", Sha256::digest(&body));
        let release = self.release(tag, asset, body.len() as u64, Some(&digest));
        self.route(
            &format!("{}/tags/{tag}", repo.releases_path()),
            Route::Body(serde_json::to_vec(&release).unwrap()),
        );
        self.route(&format!("/assets/{asset}"), Route::Body(body));
    }

    fn release(
        &self,
        tag: &str,
        asset: &str,
        size: u64,
        digest: Option<&str>,
    ) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "assets": [{
                "name": asset,
                "browser_download_url": format!("{}/assets/{asset}", self.base),
                "size": size,
                "digest": digest,
            }],
        })
    }

    fn list(&self, repo: Repo, tag: &str, asset: &str, size: u64, digest: Option<&str>) {
        let body = serde_json::json!([self.release(tag, asset, size, digest)]);
        self.route(
            repo.releases_path(),
            Route::Body(serde_json::to_vec(&body).unwrap()),
        );
    }

    fn route(&self, path: &str, route: Route) {
        self.routes.lock().unwrap().insert(path.to_owned(), route);
    }

    /// Every request line seen so far, like `GET /path?query HTTP/1.1`.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// The requests for an archive, which are the downloads.
    pub fn downloads(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|line| line.contains(" /assets/"))
            .collect()
    }

    /// The requests for a release list.
    pub fn catalog_requests(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|line| line.contains(" /repos/"))
            .collect()
    }
}

impl Drop for ReleasesApi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(
    mut stream: TcpStream,
    routes: &Mutex<HashMap<String, Route>>,
    requests: &Mutex<Vec<String>>,
    release: &AtomicBool,
    stop: &AtomicBool,
) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => request.extend_from_slice(&chunk[..read]),
        }
    }
    let text = String::from_utf8_lossy(&request);
    let line = text.lines().next().unwrap_or_default().to_owned();
    requests.lock().unwrap().push(line.clone());
    let target = line.split_whitespace().nth(1).unwrap_or_default();
    let path = target.split('?').next().unwrap_or_default();
    // Cloned out, so a long download doesn't hold up other requests.
    let route = routes.lock().unwrap().get(path).cloned();
    let reply = match route {
        Some(Route::Body(body)) => Some(Ok(body)),
        Some(Route::Status(code)) => Some(Err(code)),
        Some(Route::Drip(total)) => {
            drip(&mut stream, total, stop);
            return;
        }
        Some(Route::Stall(total)) => {
            stall(&mut stream, total, release, stop);
            return;
        }
        None => None,
    };
    let _ = match reply {
        Some(Ok(body)) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(head.as_bytes())
                .and_then(|()| stream.write_all(&body))
        }
        Some(Err(code)) => status(&mut stream, code),
        None => status(&mut stream, 404),
    };
}

fn status(stream: &mut TcpStream, code: u16) -> std::io::Result<()> {
    stream.write_all(
        format!("HTTP/1.1 {code} Reply\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
}

/// Hangs up on release, or after half a minute, so a test that forgets it
/// can't leave this running.
fn stall(stream: &mut TcpStream, total: usize, release: &AtomicBool, stop: &AtomicBool) {
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n");
    if stream.write_all(head.as_bytes()).is_err() || stream.write_all(&[0; 1024]).is_err() {
        return;
    }
    let started = Instant::now();
    while !release.load(Ordering::Relaxed)
        && !stop.load(Ordering::Relaxed)
        && started.elapsed() < Duration::from_secs(30)
    {
        thread::sleep(Duration::from_millis(10));
    }
}

/// Ends when the client hangs up, the server stops, or a minute has passed,
/// so a test that forgets it can't leave this running.
fn drip(stream: &mut TcpStream, total: usize, stop: &AtomicBool) {
    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n");
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    let started = Instant::now();
    let mut sent = 0;
    while sent < total
        && !stop.load(Ordering::Relaxed)
        && started.elapsed() < Duration::from_secs(60)
    {
        let size = 1024.min(total - sent);
        if stream.write_all(&vec![0; size]).is_err() {
            return;
        }
        sent += size;
        thread::sleep(Duration::from_millis(20));
    }
}

/// A gzipped tar of `entries`, each a path in the archive, its bytes, and its
/// mode.
fn archive(entries: &[(String, &[u8], u32)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, bytes, mode) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(*mode);
        header.set_cksum();
        builder.append_data(&mut header, path, *bytes).unwrap();
    }
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(&builder.into_inner().unwrap()).unwrap();
    gzip.finish().unwrap()
}

/// A Proton build under `root/`, with just what a download checks for.
pub fn proton_archive(root: &str) -> Vec<u8> {
    let entries: Vec<_> = ["proton", "files/bin/wine", "files/bin/wineserver"]
        .into_iter()
        .map(|file| (format!("{root}/{file}"), &b"#!/bin/sh\n"[..], 0o755))
        .collect();
    archive(&entries)
}

/// A DXVK or vkd3d-proton build under `root/`, stub DLLs for both widths.
pub fn component_archive(root: &str, dxvk: bool) -> Vec<u8> {
    let (narrow, marker) = if dxvk {
        ("x32", "dxgi.dll")
    } else {
        ("x86", "d3d12.dll")
    };
    let entries: Vec<_> = ["x64", narrow]
        .into_iter()
        .map(|arch| (format!("{root}/{arch}/{marker}"), &b"dll"[..], 0o644))
        .collect();
    archive(&entries)
}
