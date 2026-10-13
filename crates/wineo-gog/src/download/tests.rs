use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;
use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::cache::{CachedFile, write_record};
use crate::error::Class;
use crate::fake::{self, FakeGog, Reply, Request, json as reply, response, unsized_response};
use crate::product::DownloadFile;

const ACCESS: &str = "ACCESSSECRET0123456789";
const PRODUCT: u64 = 1094900565;
const DOWNLOAD: &str = "installer_windows_en";
const SETUP: &str = "setup_game_(64bit)_(123).exe";
const PART1: &str = "setup_game_(64bit)_(123)-1.bin";
const MIB: u64 = 1024 * 1024;

/// How product details list a file's size, from its real one. GOG rounds
/// to whole MiB, so the fixtures' small files are listed rounded up.
fn round_up(size: u64) -> u64 {
    size.div_ceil(MIB) * MIB
}

/// Rounded down, as a file over a MiB usually is: smaller than it is.
fn round_down(size: u64) -> u64 {
    size / MIB * MIB
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sum {
    Right,
    Wrong,
    NoUrl,
    Missing,
    Junk,
    WrongTotal,
    /// A total five bytes short of the file.
    ShortTotal,
    /// A 500 to every checksum request.
    ServerError,
    /// A 500 to the first checksum request, then the right one.
    Flaky,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ranges {
    Honor,
    Whole,
    WrongStart,
    WrongTotal,
    /// A 206 with a range that never fits, even without a Range request.
    AlwaysWrong,
    /// A 416 to any Range request.
    Unsatisfiable,
    /// A 416 to every request.
    AlwaysUnsatisfiable,
}

struct Script {
    /// By file id: the file's name and contents.
    files: BTreeMap<String, (String, Vec<u8>)>,
    sum: Sum,
    ranges: Ranges,
    resolves: u32,
    /// Links from this many resolves and earlier are refused.
    expired_through: u32,
    refuse_with: u16,
    refuse_always: bool,
    /// Whether expired links refuse checksum requests too, not just files.
    refuse_checksum: bool,
    checksum_requests: u32,
    resolve_status: Option<u16>,
    file_status: Option<u16>,
    /// Links go through a redirect to the real file.
    redirect: bool,
    /// Where redirects go, when not back to this server.
    redirect_to: Option<String>,
    /// Whole files are sent without a `Content-Length`.
    no_length: bool,
}

struct Gog {
    server: FakeGog,
    script: Arc<Mutex<Script>>,
    _dir: TempDir,
    cache: DownloadCache,
    /// The installer's file list, as product details would give it.
    siblings: Vec<DownloadFile>,
    listing: fn(u64) -> u64,
}

fn body(len: usize, seed: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + seed) % 251) as u8).collect()
}

fn md5_of(bytes: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(bytes);
    cache::hex(hasher)
}

/// Links carry names percent-encoded, as GOG's do.
fn encode(name: &str) -> String {
    name.replace('(', "%28").replace(')', "%29")
}

fn handle(script: &Mutex<Script>, request: &Request) -> Reply {
    let mut script = script.lock().unwrap();
    let host = request.header("host").unwrap_or_default().to_owned();
    let path = request.path().to_owned();
    if let Some(file_id) = path.strip_prefix(&format!("/products/{PRODUCT}/downlink/installer/")) {
        script.resolves += 1;
        let n = script.resolves;
        if let Some(status) = script.resolve_status {
            return reply(status, &format!("{{\"echo\":\"{ACCESS}\"}}"));
        }
        let Some((name, _)) = script.files.get(file_id) else {
            return reply(404, "{}");
        };
        let kind = if script.redirect { "redirect" } else { "cdn" };
        let link = format!(
            "http://{host}/{kind}/{n}/{}?token=SIGNED{n}&hmac=SIGNATURE",
            encode(name)
        );
        let checksum = (script.sum != Sum::NoUrl)
            .then(|| format!("http://{host}/cdn/{n}/{}.xml?token=SIGNED{n}", encode(name)));
        return reply(
            200,
            &json!({"downlink": link, "checksum": checksum}).to_string(),
        );
    }
    if let Some(rest) = path.strip_prefix("/redirect/") {
        let base = script
            .redirect_to
            .clone()
            .unwrap_or_else(|| format!("http://{host}"));
        let location = format!("{base}/cdn/{rest}?token=SIGNED");
        return Reply::Send(response(302, &[("Location", location)], &[]));
    }
    let Some(rest) = path.strip_prefix("/cdn/") else {
        return reply(404, "{}");
    };
    let (n, segment) = rest.split_once('/').unwrap();
    let n: u32 = n.parse().unwrap();
    let checksum = segment.ends_with(".xml");
    if (script.refuse_always || n <= script.expired_through)
        && (script.refuse_checksum || !checksum)
    {
        let status = script.refuse_with;
        return Reply::Send(response(status, &[], b"expired SIGNED"));
    }
    if let Some(segment) = segment.strip_suffix(".xml") {
        script.checksum_requests += 1;
        let Some((name, body)) = script
            .files
            .values()
            .find(|(name, _)| encode(name) == segment)
        else {
            return reply(404, "{}");
        };
        let (md5, total) = match script.sum {
            Sum::Wrong => (md5_of(b"something else"), body.len()),
            Sum::WrongTotal => (md5_of(body), body.len() + 5),
            Sum::ShortTotal => (md5_of(body), body.len() - 5),
            _ => (md5_of(body), body.len()),
        };
        return match script.sum {
            Sum::Missing => reply(404, ""),
            Sum::Junk => Reply::Send(response(200, &[], b"<html>no</html>")),
            Sum::ServerError => reply(500, ""),
            Sum::Flaky if script.checksum_requests == 1 => reply(500, ""),
            _ => Reply::Send(response(
                200,
                &[],
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<file name=\"{name}\" available=\"1\" notavailablemsg=\"\" md5=\"{md5}\" chunks=\"1\" timestamp=\"2024-01-02 03:04:05\" total_size=\"{total}\">\n\t<chunk id=\"0\" from=\"0\" to=\"{}\" method=\"md5\">{md5}</chunk>\n</file>\n",
                    total - 1
                )
                .as_bytes(),
            )),
        };
    }
    if let Some(status) = script.file_status {
        return Reply::Send(response(status, &[], b"SIGNED"));
    }
    let Some((_, body)) = script
        .files
        .values()
        .find(|(name, _)| encode(name) == segment)
    else {
        return reply(404, "{}");
    };
    let start = request
        .header("range")
        .and_then(|range| range.strip_prefix("bytes="))
        .and_then(|range| range.strip_suffix('-'))
        .and_then(|start| start.parse::<usize>().ok());
    match (script.ranges, start) {
        (Ranges::AlwaysWrong, _) => Reply::Send(response(
            206,
            &[("Content-Range", format!("bytes 5-9/{}", body.len()))],
            &body[5..10],
        )),
        (Ranges::AlwaysUnsatisfiable, _) | (Ranges::Unsatisfiable, Some(_)) => {
            Reply::Send(response(
                416,
                &[("Content-Range", format!("bytes */{}", body.len()))],
                &[],
            ))
        }
        (_, None) if script.no_length => Reply::Send(unsized_response(200, &[], body)),
        (Ranges::Honor, _) | (_, None) => fake::file(request, body),
        (Ranges::Whole, Some(_)) => Reply::Send(response(200, &[], body)),
        (Ranges::WrongStart, Some(start)) => Reply::Send(response(
            206,
            &[(
                "Content-Range",
                format!("bytes {}-{}/{}", start - 1, body.len() - 1, body.len()),
            )],
            &body[start - 1..],
        )),
        (Ranges::WrongTotal, Some(start)) => Reply::Send(response(
            206,
            &[(
                "Content-Range",
                format!("bytes {start}-{}/{}", body.len() - 1, body.len() + 1),
            )],
            &body[start..],
        )),
    }
}

impl Gog {
    fn start(files: &[(&str, &str, Vec<u8>)]) -> Self {
        Self::start_listed(files, round_up)
    }

    fn start_listed(files: &[(&str, &str, Vec<u8>)], listing: fn(u64) -> u64) -> Self {
        let script = Arc::new(Mutex::new(Script {
            files: files
                .iter()
                .map(|(id, name, body)| ((*id).to_owned(), ((*name).to_owned(), body.clone())))
                .collect(),
            sum: Sum::Right,
            ranges: Ranges::Honor,
            resolves: 0,
            expired_through: 0,
            refuse_with: 403,
            refuse_always: false,
            refuse_checksum: true,
            checksum_requests: 0,
            resolve_status: None,
            file_status: None,
            redirect: false,
            redirect_to: None,
            no_length: false,
        }));
        let handler = Arc::clone(&script);
        let server = FakeGog::start(move |request| handle(&handler, request));
        let dir = TempDir::new().unwrap();
        let cache = DownloadCache::new(dir.path().join("cache"));
        Self {
            server,
            script,
            _dir: dir,
            cache,
            siblings: files
                .iter()
                .map(|(id, _, body)| DownloadFile {
                    id: (*id).to_owned(),
                    size: listing(body.len() as u64),
                    downlink: String::new(),
                })
                .collect(),
            listing,
        }
    }

    /// One installer file of 1.5 MiB and a bit, listed rounded down.
    fn big() -> Self {
        Self::start_listed(
            &[("en1installer0", SETUP, body(MIB as usize * 3 / 2 + 123, 0))],
            round_down,
        )
    }

    /// One installer file of 600 KB, a few chunks long.
    fn one() -> Self {
        Self::start(&[("en1installer0", SETUP, body(600_000, 0))])
    }

    fn set(&self, change: impl FnOnce(&mut Script)) {
        change(&mut self.script.lock().unwrap());
    }

    fn body(&self, file_id: &str) -> Vec<u8> {
        self.script.lock().unwrap().files[file_id].1.clone()
    }

    /// The file's real size, which the listing only rounds.
    fn size(&self, file_id: &str) -> u64 {
        self.body(file_id).len() as u64
    }

    fn package<'a>(&'a self, file_id: &'a str) -> Package<'a> {
        Package {
            product_id: PRODUCT,
            download_id: DOWNLOAD,
            file_id,
            size: (self.listing)(self.size(file_id)),
            siblings: &self.siblings,
        }
    }

    fn folder(&self) -> PathBuf {
        self.cache.dir().join(PRODUCT.to_string()).join(DOWNLOAD)
    }

    fn get(&self, package: &Package<'_>) -> Result<Downloaded, Error> {
        self.get_with(package, &|| false, &|_, _| {})
    }

    fn get_with(
        &self,
        package: &Package<'_>,
        cancel: &dyn Fn() -> bool,
        progress: &dyn Fn(u64, u64),
    ) -> Result<Downloaded, Error> {
        let downlink = format!(
            "{}/products/{PRODUCT}/downlink/installer/{}",
            self.server.base, package.file_id
        );
        self.server
            .client()
            .download(ACCESS, &self.cache, package, &downlink, cancel, progress)
    }

    fn file_requests(&self) -> Vec<Request> {
        self.server
            .requests()
            .into_iter()
            .filter(|request| {
                request.path().starts_with("/cdn/") && !request.path().ends_with(".xml")
            })
            .collect()
    }

    /// A partial file left by an earlier attempt, with its record.
    fn leave_partial(&self, record: Record, bytes: &[u8]) {
        fs::create_dir_all(self.folder()).unwrap();
        fs::write(self.folder().join(format!("{}.part", record.name)), bytes).unwrap();
        write_record(&self.folder(), &record).unwrap();
    }

    fn record(&self, file_id: &str) -> Record {
        let (name, body) = self.script.lock().unwrap().files[file_id].clone();
        Record {
            file_id: file_id.to_owned(),
            name,
            size: body.len() as u64,
            md5: Some(md5_of(&body)),
        }
    }

    fn product_entries(&self) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(self.cache.dir().join(PRODUCT.to_string()))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(self.folder())
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

/// Every way an error could be shown, which must hold no secret, link, or
/// file name.
fn assert_clean(error: &Error) {
    let shown = format!("{error} {error:?} {error:#?} {}", error.kind());
    for secret in [
        ACCESS,
        "SIGNED",
        "SIGNATURE",
        "hmac",
        "token",
        "?",
        "127.0.0.1",
        "/cdn",
        "setup_game",
    ] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
}

#[test]
fn a_full_download_lands_under_its_own_name_and_is_verified() {
    let gog = Gog::one();
    let seen = Mutex::new(Vec::new());
    let package = gog.package("en1installer0");
    let downloaded = gog
        .get_with(&package, &|| false, &|have, total| {
            seen.lock().unwrap().push((have, total))
        })
        .unwrap();
    let body = gog.body("en1installer0");
    assert_eq!(downloaded.path, gog.folder().join(SETUP));
    assert_eq!(fs::read(&downloaded.path).unwrap(), body);
    assert_eq!(downloaded.size, body.len() as u64);
    assert_eq!(downloaded.verification, Verification::Md5(md5_of(&body)));
    assert!(!downloaded.reused);
    assert_eq!(gog.names(), [".en1installer0.json", SETUP]);

    let seen = seen.into_inner().unwrap();
    let total = body.len() as u64;
    assert_eq!(seen.first(), Some(&(0, total)));
    assert_eq!(seen.last(), Some(&(total, total)));
    assert!(seen.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    assert!(seen.len() > 2, "{seen:?}");

    let requests = gog.server.requests();
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {ACCESS}").as_str())
    );
    assert_eq!(requests.len(), 3);
    for request in &requests[1..] {
        assert!(request.path().starts_with("/cdn/1/"));
        assert_eq!(request.header("authorization"), None);
        assert_eq!(request.header("range"), None);
    }
}

#[test]
fn a_partial_file_resumes_from_its_end_with_a_fresh_link() {
    let gog = Gog::one();
    let body = gog.body("en1installer0");
    gog.leave_partial(gog.record("en1installer0"), &body[..1000]);
    let seen = Mutex::new(Vec::new());
    let downloaded = gog
        .get_with(&gog.package("en1installer0"), &|| false, &|have, _| {
            seen.lock().unwrap().push(have)
        })
        .unwrap();
    assert_eq!(fs::read(&downloaded.path).unwrap(), body);
    assert_eq!(downloaded.verification, Verification::Md5(md5_of(&body)));
    assert_eq!(seen.lock().unwrap()[0], 1000);
    let files = gog.file_requests();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].header("range"), Some("bytes=1000-"));
    assert_eq!(gog.names(), [".en1installer0.json", SETUP]);
}

#[test]
fn a_whole_file_answering_a_range_starts_over() {
    let gog = Gog::one();
    gog.set(|script| script.ranges = Ranges::Whole);
    // Bytes that don't belong to the file, so only a restart can verify.
    gog.leave_partial(gog.record("en1installer0"), &[7; 1000]);
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert_eq!(
        fs::read(&downloaded.path).unwrap(),
        gog.body("en1installer0")
    );
    assert_eq!(gog.file_requests()[0].header("range"), Some("bytes=1000-"));
}

#[test]
fn a_wrong_content_range_starts_over_from_zero() {
    for ranges in [Ranges::WrongStart, Ranges::WrongTotal] {
        let gog = Gog::one();
        gog.set(|script| script.ranges = ranges);
        let body = gog.body("en1installer0");
        gog.leave_partial(gog.record("en1installer0"), &body[..1000]);
        let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
        assert_eq!(fs::read(&downloaded.path).unwrap(), body, "{ranges:?}");
        let files = gog.file_requests();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].header("range"), Some("bytes=1000-"));
        assert_eq!(files[1].header("range"), None);
        assert_eq!(gog.script.lock().unwrap().resolves, 2);
    }
}

#[test]
fn a_server_that_keeps_sending_the_wrong_range_fails_without_a_partial() {
    let gog = Gog::one();
    gog.set(|script| script.ranges = Ranges::AlwaysWrong);
    let body = gog.body("en1installer0");
    gog.leave_partial(gog.record("en1installer0"), &body[..1000]);
    let error = gog.get(&gog.package("en1installer0")).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind(), error.class()),
        (Endpoint::Download, ErrorKind::WrongRange, Class::Malformed)
    );
    assert_clean(&error);
    let files = gog.file_requests();
    assert_eq!(files.len(), 2);
    assert_eq!(files[1].header("range"), None);
    // Nothing is left to resume from, so the next try starts clean.
    assert!(gog.names().is_empty(), "{:?}", gog.names());
}

#[test]
fn a_refused_link_is_resolved_again_once() {
    for (status, refuse_checksum) in [(401, false), (403, false), (401, true), (403, true)] {
        let gog = Gog::one();
        gog.set(|script| {
            script.expired_through = 1;
            script.refuse_with = status;
            script.refuse_checksum = refuse_checksum;
        });
        let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
        assert_eq!(
            fs::read(&downloaded.path).unwrap(),
            gog.body("en1installer0")
        );
        assert!(matches!(downloaded.verification, Verification::Md5(_)));
        assert_eq!(gog.script.lock().unwrap().resolves, 2);
        // A refused checksum stops the attempt before the file is asked for.
        let files = gog.file_requests();
        assert_eq!(files.len(), if refuse_checksum { 1 } else { 2 });
        assert!(files.last().unwrap().path().starts_with("/cdn/2/"));
    }
}

#[test]
fn a_link_refused_twice_fails_without_blaming_the_login() {
    for status in [401, 403] {
        let gog = Gog::one();
        gog.set(|script| {
            script.refuse_always = true;
            script.refuse_with = status;
        });
        let error = gog.get(&gog.package("en1installer0")).unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind(), error.class()),
            (Endpoint::Download, ErrorKind::Status(status), Class::Other)
        );
        assert_clean(&error);
        assert_eq!(gog.script.lock().unwrap().resolves, 2);
    }
}

#[test]
fn a_refused_token_on_the_link_is_unauthorized_so_the_caller_can_renew() {
    let gog = Gog::one();
    gog.set(|script| script.resolve_status = Some(401));
    let error = gog.get(&gog.package("en1installer0")).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Link, ErrorKind::Unauthorized)
    );
    assert_clean(&error);
    assert!(gog.file_requests().is_empty());
}

#[test]
fn a_canceled_download_resumes_later_from_a_fresh_link() {
    let gog = Gog::one();
    let started = Cell::new(false);
    let error = gog
        .get_with(
            &gog.package("en1installer0"),
            &|| started.get(),
            &|have, _| started.set(have > 0),
        )
        .unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Download, ErrorKind::Canceled)
    );
    let body = gog.body("en1installer0");
    let part = fs::read(gog.folder().join(format!("{SETUP}.part"))).unwrap();
    assert!(
        !part.is_empty() && part.len() < body.len(),
        "{}",
        part.len()
    );
    assert_eq!(part, &body[..part.len()]);
    assert_eq!(
        gog.names(),
        [".en1installer0.json".to_owned(), format!("{SETUP}.part")]
    );

    // The old link has expired since.
    gog.set(|script| script.expired_through = script.resolves);
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert_eq!(fs::read(&downloaded.path).unwrap(), body);
    let files = gog.file_requests();
    assert_eq!(files.len(), 2);
    assert!(files[1].path().starts_with("/cdn/2/"));
    assert_eq!(
        files[1].header("range"),
        Some(format!("bytes={}-", part.len()).as_str())
    );
    assert_eq!(gog.names(), [".en1installer0.json", SETUP]);
}

#[test]
fn a_checksum_mismatch_removes_the_file() {
    let gog = Gog::one();
    gog.set(|script| script.sum = Sum::Wrong);
    let error = gog.get(&gog.package("en1installer0")).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Download, ErrorKind::WrongChecksum)
    );
    assert_clean(&error);
    assert!(gog.names().is_empty(), "{:?}", gog.names());
}

#[test]
fn without_a_checksum_link_only_the_size_is_checked() {
    let gog = Gog::one();
    gog.set(|script| script.sum = Sum::NoUrl);
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert_eq!(downloaded.verification, Verification::SizeOnly);
    assert_eq!(
        fs::read(&downloaded.path).unwrap(),
        gog.body("en1installer0")
    );
    assert_eq!(gog.script.lock().unwrap().checksum_requests, 0);
}

#[test]
fn a_published_checksum_that_can_t_be_had_fails_and_keeps_the_partial() {
    for (sum, kind, class) in [
        (Sum::Missing, ErrorKind::Status(404), Class::Other),
        (Sum::Junk, ErrorKind::Malformed, Class::Malformed),
        (Sum::ServerError, ErrorKind::Server(500), Class::Temporary),
    ] {
        let gog = Gog::one();
        gog.set(|script| script.sum = sum);
        let body = gog.body("en1installer0");
        gog.leave_partial(gog.record("en1installer0"), &body[..1000]);
        let error = gog.get(&gog.package("en1installer0")).unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind(), error.class()),
            (Endpoint::Checksum, kind, class),
            "{sum:?}"
        );
        assert_clean(&error);
        // Tried once more before giving up, and the file never asked for.
        assert_eq!(gog.script.lock().unwrap().checksum_requests, 2);
        assert!(gog.file_requests().is_empty());
        let part = fs::read(gog.folder().join(format!("{SETUP}.part"))).unwrap();
        assert_eq!(part, &body[..1000]);
    }
}

#[test]
fn a_checksum_that_fails_once_is_tried_again() {
    let gog = Gog::one();
    gog.set(|script| script.sum = Sum::Flaky);
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert!(matches!(downloaded.verification, Verification::Md5(_)));
    assert_eq!(gog.script.lock().unwrap().checksum_requests, 2);
}

#[test]
fn a_listing_off_by_rounding_still_downloads_at_the_exact_size() {
    for sum in [Sum::Right, Sum::NoUrl] {
        for offset in [10, -10, MIB as i64 - 1, -500_000] {
            let gog = Gog::one();
            gog.set(|script| script.sum = sum);
            let size = gog.size("en1installer0");
            let package = Package {
                size: size.saturating_add_signed(offset),
                ..gog.package("en1installer0")
            };
            let downloaded = gog.get(&package).unwrap();
            assert_eq!(downloaded.size, size, "{sum:?} {offset}");
            assert_eq!(
                fs::read(&downloaded.path).unwrap(),
                gog.body("en1installer0")
            );
            let record = cache::read_record(&gog.folder(), "en1installer0").unwrap();
            assert_eq!(record.size, size);
        }
    }
}

#[test]
fn sizes_gog_s_own_answers_disagree_on_fail() {
    for (sum, no_length) in [
        (Sum::WrongTotal, false),
        (Sum::ShortTotal, false),
        // Only the body itself shows it runs past the checksum's total.
        (Sum::ShortTotal, true),
    ] {
        let gog = Gog::one();
        gog.set(|script| {
            script.sum = sum;
            script.no_length = no_length;
        });
        let error = gog.get(&gog.package("en1installer0")).unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind()),
            (Endpoint::Download, ErrorKind::WrongSize),
            "{sum:?} {no_length}"
        );
        assert_clean(&error);
        assert!(!gog.folder().join(SETUP).exists());
    }
}

#[test]
fn a_rounded_down_listing_with_a_checksum_gets_the_exact_size() {
    let gog = Gog::big();
    let size = gog.size("en1installer0");
    let package = gog.package("en1installer0");
    assert!(package.size < size);
    let seen = Mutex::new(Vec::new());
    let downloaded = gog
        .get_with(&package, &|| false, &|have, total| {
            seen.lock().unwrap().push((have, total))
        })
        .unwrap();
    assert_eq!(downloaded.size, size);
    assert_eq!(
        fs::read(&downloaded.path).unwrap(),
        gog.body("en1installer0")
    );
    assert!(matches!(downloaded.verification, Verification::Md5(_)));
    let seen = seen.into_inner().unwrap();
    assert_eq!(seen.first(), Some(&(0, size)));
    assert_eq!(seen.last(), Some(&(size, size)));
    let files = gog.cache.files(PRODUCT).unwrap();
    assert_eq!((files[0].size, files[0].complete), (size, true));

    // Found again from the rounded listing, and the journal's exact size.
    let again = gog.get(&package).unwrap();
    assert!(again.reused);
    assert_eq!(again.size, size);
    let exact = Package { size, ..package };
    assert!(
        gog.cache
            .complete(&exact, None, &|| false)
            .unwrap()
            .is_some()
    );
}

#[test]
fn without_a_checksum_the_length_gives_the_exact_size() {
    let gog = Gog::big();
    gog.set(|script| script.sum = Sum::NoUrl);
    let size = gog.size("en1installer0");
    let package = gog.package("en1installer0");
    let seen = Mutex::new(Vec::new());
    let downloaded = gog
        .get_with(&package, &|| false, &|have, total| {
            seen.lock().unwrap().push((have, total))
        })
        .unwrap();
    assert_eq!(downloaded.size, size);
    assert_eq!(downloaded.verification, Verification::SizeOnly);
    assert_eq!(
        fs::read(&downloaded.path).unwrap(),
        gog.body("en1installer0")
    );
    let seen = seen.into_inner().unwrap();
    // The listed size until the server's answer gives the exact one.
    assert_eq!(seen.first(), Some(&(0, package.size)));
    assert_eq!(seen.last(), Some(&(size, size)));
    let record = cache::read_record(&gog.folder(), "en1installer0").unwrap();
    assert_eq!((record.size, record.md5), (size, None));
    assert!(gog.get(&package).unwrap().reused);
}

#[test]
fn a_download_with_a_rounded_listing_resumes_after_an_interruption() {
    for sum in [Sum::Right, Sum::NoUrl] {
        let gog = Gog::big();
        gog.set(|script| script.sum = sum);
        let package = gog.package("en1installer0");
        let started = Cell::new(false);
        let error = gog
            .get_with(&package, &|| started.get(), &|have, _| {
                started.set(have > 0)
            })
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Canceled, "{sum:?}");
        let part = fs::read(gog.folder().join(format!("{SETUP}.part"))).unwrap();
        assert!(!part.is_empty(), "{sum:?}");
        let record = cache::read_record(&gog.folder(), "en1installer0").unwrap();
        assert_eq!(record.size, gog.size("en1installer0"), "{sum:?}");

        let downloaded = gog.get(&package).unwrap();
        assert_eq!(downloaded.size, gog.size("en1installer0"));
        assert_eq!(
            fs::read(&downloaded.path).unwrap(),
            gog.body("en1installer0")
        );
        let files = gog.file_requests();
        assert_eq!(files.len(), 2, "{sum:?}");
        assert_eq!(
            files[1].header("range"),
            Some(format!("bytes={}-", part.len()).as_str()),
            "{sum:?}"
        );
    }
}

#[test]
fn a_whole_partial_file_without_a_checksum_is_not_fetched_again() {
    let gog = Gog::big();
    gog.set(|script| script.sum = Sum::NoUrl);
    let record = Record {
        md5: None,
        ..gog.record("en1installer0")
    };
    gog.leave_partial(record, &gog.body("en1installer0"));
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert_eq!(downloaded.size, gog.size("en1installer0"));
    assert_eq!(downloaded.verification, Verification::SizeOnly);
    assert!(gog.file_requests().is_empty());
    assert_eq!(gog.names(), [".en1installer0.json", SETUP]);
}

#[test]
fn a_record_far_from_the_listing_without_a_checksum_starts_over() {
    let gog = Gog::big();
    gog.set(|script| script.sum = Sum::NoUrl);
    let record = Record {
        size: gog.size("en1installer0") + 2 * MIB,
        md5: None,
        ..gog.record("en1installer0")
    };
    gog.leave_partial(record, &gog.body("en1installer0")[..1000]);
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert_eq!(downloaded.size, gog.size("en1installer0"));
    assert_eq!(gog.file_requests()[0].header("range"), None);
}

#[test]
fn a_legacy_record_with_the_listed_size_gets_the_exact_one() {
    // What the old code left for a file with no checksum: a record holding
    // the rounded listing and an empty partial file.
    let gog = Gog::big();
    gog.set(|script| script.sum = Sum::NoUrl);
    let package = gog.package("en1installer0");
    let record = Record {
        size: package.size,
        md5: None,
        ..gog.record("en1installer0")
    };
    gog.leave_partial(record, &[]);
    let downloaded = gog.get(&package).unwrap();
    let size = gog.size("en1installer0");
    assert_eq!(downloaded.size, size);
    assert_eq!(
        fs::read(&downloaded.path).unwrap(),
        gog.body("en1installer0")
    );
    let record = cache::read_record(&gog.folder(), "en1installer0").unwrap();
    assert_eq!((record.size, record.md5), (size, None));
    assert_eq!(gog.file_requests()[0].header("range"), None);
}

#[test]
fn a_whole_file_of_another_length_replaces_a_record_without_a_checksum() {
    let gog = Gog::big();
    gog.set(|script| {
        script.sum = Sum::NoUrl;
        script.ranges = Ranges::Whole;
    });
    let size = gog.size("en1installer0");
    let record = Record {
        size: size + 1000,
        md5: None,
        ..gog.record("en1installer0")
    };
    gog.leave_partial(record, &[7; 1000]);
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert_eq!(downloaded.size, size);
    assert_eq!(
        fs::read(&downloaded.path).unwrap(),
        gog.body("en1installer0")
    );
    let record = cache::read_record(&gog.folder(), "en1installer0").unwrap();
    assert_eq!(record.size, size);
    let files = gog.file_requests();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].header("range"), Some("bytes=1000-"));
}

#[test]
fn a_file_of_no_known_size_is_refused() {
    let gog = Gog::big();
    gog.set(|script| {
        script.sum = Sum::NoUrl;
        script.no_length = true;
    });
    let error = gog.get(&gog.package("en1installer0")).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind(), error.class()),
        (Endpoint::Download, ErrorKind::UnknownSize, Class::Malformed)
    );
    assert_clean(&error);
    assert!(gog.names().is_empty(), "{:?}", gog.names());

    // GOG's checksum gives the size the server didn't.
    gog.set(|script| script.sum = Sum::Right);
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert_eq!(downloaded.size, gog.size("en1installer0"));
    assert_eq!(
        fs::read(&downloaded.path).unwrap(),
        gog.body("en1installer0")
    );
}

#[test]
fn a_record_for_a_different_file_starts_over() {
    let changes: [fn(&mut Record); 3] = [
        |record| record.size += 1,
        |record| record.md5 = Some(md5_of(b"older build")),
        |record| record.name = "setup_game_(64bit)_(100).exe".to_owned(),
    ];
    for change in changes {
        let gog = Gog::one();
        let mut record = gog.record("en1installer0");
        change(&mut record);
        gog.leave_partial(record.clone(), &[7; 1000]);
        // A finished file from the old record goes too.
        fs::write(gog.folder().join(&record.name), b"old").unwrap();
        let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
        assert_eq!(
            fs::read(&downloaded.path).unwrap(),
            gog.body("en1installer0")
        );
        assert_eq!(gog.file_requests()[0].header("range"), None);
        assert_eq!(gog.names(), [".en1installer0.json", SETUP]);
    }
}

#[test]
fn a_complete_file_is_reused_without_any_request() {
    let gog = Gog::one();
    let package = gog.package("en1installer0");
    let first = gog.get(&package).unwrap();
    let requests = gog.server.requests().len();
    let seen = Cell::new((0, 0));
    let again = gog
        .get_with(&package, &|| false, &|have, total| seen.set((have, total)))
        .unwrap();
    assert!(again.reused);
    assert_eq!(
        (again.path, again.verification),
        (first.path.clone(), first.verification.clone())
    );
    let size = gog.size("en1installer0");
    assert_eq!(seen.get(), (size, size));
    assert_eq!(gog.server.requests().len(), requests);

    let md5 = md5_of(&gog.body("en1installer0"));
    let complete = |md5: Option<&str>| gog.cache.complete(&package, md5, &|| false).unwrap();
    assert_eq!(complete(None), Some(first.path.clone()));
    assert_eq!(
        complete(Some(&md5.to_ascii_uppercase())),
        Some(first.path.clone())
    );
    assert_eq!(complete(Some(&md5_of(b"other"))), None);
    // Further from the listed size than GOG's rounding goes.
    let other_size = Package {
        size: size + 2 * MIB,
        ..package
    };
    assert_eq!(
        gog.cache.complete(&other_size, None, &|| false).unwrap(),
        None
    );

    // Damaged in place, at the same size: no longer complete, so it's
    // downloaded again.
    let mut bytes = fs::read(&first.path).unwrap();
    bytes[10] ^= 1;
    fs::write(&first.path, &bytes).unwrap();
    assert_eq!(complete(None), None);
    let repaired = gog.get(&package).unwrap();
    assert!(!repaired.reused);
    assert_eq!(fs::read(&repaired.path).unwrap(), gog.body("en1installer0"));
}

#[test]
fn hashing_a_cached_file_can_be_canceled() {
    let gog = Gog::one();
    let package = gog.package("en1installer0");
    gog.get(&package).unwrap();
    let error = gog.cache.complete(&package, None, &|| true).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Cache, ErrorKind::Canceled)
    );
}

#[test]
fn a_multi_part_installer_lands_in_one_folder_under_its_own_names() {
    let gog = Gog::start(&[
        ("en1installer0", SETUP, body(300_000, 1)),
        ("en1installer1", PART1, body(400_000, 2)),
    ]);
    let setup = gog.get(&gog.package("en1installer0")).unwrap();
    let part = gog.get(&gog.package("en1installer1")).unwrap();
    assert_eq!(setup.path.parent(), part.path.parent());
    assert_eq!(fs::read(&part.path).unwrap(), gog.body("en1installer1"));
    assert_eq!(
        gog.names(),
        [".en1installer0.json", ".en1installer1.json", PART1, SETUP]
    );

    let files = gog.cache.files(PRODUCT).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(
        files[1],
        CachedFile {
            download_id: DOWNLOAD.to_owned(),
            file_id: "en1installer1".to_owned(),
            name: PART1.to_owned(),
            path: part.path.clone(),
            size: 400_000,
            md5: Some(md5_of(&gog.body("en1installer1"))),
            complete: true,
        }
    );
    assert!(files[0].complete);
    assert_eq!(gog.cache.files(7).unwrap(), []);

    gog.cache.remove_product(PRODUCT, &|| false).unwrap();
    assert_eq!(gog.cache.files(PRODUCT).unwrap(), []);
    assert_eq!(gog.product_entries(), [".lock"]);
    gog.cache.remove_product(PRODUCT, &|| false).unwrap();
}

#[test]
fn two_files_of_one_installer_with_one_name_are_refused() {
    let gog = Gog::start(&[
        ("en1installer0", SETUP, body(1000, 1)),
        ("en1installer1", SETUP, body(2000, 2)),
    ]);
    gog.get(&gog.package("en1installer0")).unwrap();
    let error = gog.get(&gog.package("en1installer1")).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Download, ErrorKind::Malformed)
    );
    // The first file is untouched.
    assert_eq!(
        fs::read(gog.folder().join(SETUP)).unwrap(),
        gog.body("en1installer0")
    );
}

#[test]
fn a_partial_file_is_listed_as_incomplete() {
    let gog = Gog::one();
    gog.leave_partial(gog.record("en1installer0"), &[1; 10]);
    let files = gog.cache.files(PRODUCT).unwrap();
    assert_eq!(files.len(), 1);
    assert!(!files[0].complete);
    assert_eq!(
        gog.cache
            .complete(&gog.package("en1installer0"), None, &|| false)
            .unwrap(),
        None
    );
}

#[test]
fn redirects_from_the_download_server_are_followed() {
    let gog = Gog::one();
    gog.set(|script| script.redirect = true);
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert_eq!(
        fs::read(&downloaded.path).unwrap(),
        gog.body("en1installer0")
    );
    let redirected: Vec<_> = gog
        .server
        .requests()
        .into_iter()
        .filter(|request| !request.path().starts_with("/products/"))
        .collect();
    assert_eq!(redirected.len(), 3);
    assert!(
        redirected
            .iter()
            .all(|request| request.header("authorization").is_none())
    );
}

#[test]
fn a_redirect_to_another_host_gets_neither_the_token_nor_the_signed_link() {
    let file = body(600_000, 0);
    let served = file.clone();
    let other = FakeGog::start(move |request| fake::file(request, &served));
    let gog = Gog::one();
    gog.set(|script| {
        script.redirect = true;
        script.redirect_to = Some(other.base.clone());
    });
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert_eq!(fs::read(&downloaded.path).unwrap(), file);
    let requests = other.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].header("authorization"), None);
    assert_eq!(requests[0].header("referer"), None);
}

#[test]
fn unsafe_ids_and_names_are_refused() {
    let gog = Gog::one();
    for (download_id, file_id) in [("../up", "en1installer0"), (DOWNLOAD, ".hidden"), ("", "x")] {
        let package = Package {
            download_id,
            file_id,
            ..gog.package("en1installer0")
        };
        let error = gog.get(&package).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Malformed);
    }
    let empty = Package {
        size: 0,
        ..gog.package("en1installer0")
    };
    assert_eq!(gog.get(&empty).unwrap_err().kind(), ErrorKind::Malformed);
    assert!(gog.server.requests().is_empty());

    for name in [
        ".hidden.exe",
        "setup.exe.part",
        "a%2Fb.exe",
        "bad%zz.exe",
        "%C3%28.exe",
    ] {
        let gog = Gog::start(&[("en1installer0", name, body(10, 0))]);
        let error = gog.get(&gog.package("en1installer0")).unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind()),
            (Endpoint::Link, ErrorKind::Malformed),
            "{name}"
        );
        assert!(!gog.folder().exists());
    }
}

#[test]
fn no_download_error_shows_a_secret_or_a_signed_link() {
    let setups: [fn(&mut Script); 11] = [
        |script| script.resolve_status = Some(401),
        |script| script.resolve_status = Some(500),
        |script| script.file_status = Some(500),
        |script| script.file_status = Some(404),
        |script| script.file_status = Some(429),
        |script| {
            script.refuse_always = true;
            script.refuse_with = 403;
        },
        |script| script.sum = Sum::Wrong,
        |script| script.sum = Sum::WrongTotal,
        |script| script.sum = Sum::Missing,
        |script| script.sum = Sum::Junk,
        |script| script.ranges = Ranges::AlwaysUnsatisfiable,
    ];
    for setup in setups {
        let gog = Gog::one();
        gog.set(setup);
        let error = gog.get(&gog.package("en1installer0")).unwrap_err();
        assert_clean(&error);
    }
    let gog = Gog::one();
    gog.set(|script| script.file_status = Some(503));
    let error = gog.get(&gog.package("en1installer0")).unwrap_err();
    assert_eq!(
        (error.kind(), error.class()),
        (ErrorKind::Server(503), Class::Temporary)
    );

    let base = fake::refused();
    let client = Client::with_base("wineo-test/1", &base).unwrap();
    let dir = TempDir::new().unwrap();
    let cache = DownloadCache::new(dir.path());
    let gog = Gog::one();
    let error = client
        .download(
            ACCESS,
            &cache,
            &gog.package("en1installer0"),
            &format!("{base}/products/{PRODUCT}/downlink/installer/x"),
            &|| false,
            &|_, _| {},
        )
        .unwrap_err();
    assert_eq!(error.class(), Class::Temporary);
    assert_clean(&error);
}

#[test]
fn content_ranges_and_encoded_names_parse_strictly() {
    assert_eq!(content_range("bytes 10-19/20"), Some((10, 19, 20)));
    for bad in [
        "bytes 10-19",
        "bytes */20",
        "items 1-2/3",
        "bytes -1-2/3",
        "bytes 1-2/x",
    ] {
        assert_eq!(content_range(bad), None, "{bad}");
    }
    assert_eq!(percent_decode("a%28b%29").as_deref(), Some("a(b)"));
    for bad in ["%", "%2", "%+f", "%zz", "%ff"] {
        assert_eq!(percent_decode(bad), None, "{bad}");
    }
}

#[test]
fn a_partial_file_already_whole_is_verified_without_fetching_it() {
    let gog = Gog::one();
    gog.leave_partial(gog.record("en1installer0"), &gog.body("en1installer0"));
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert!(!downloaded.reused);
    assert!(matches!(downloaded.verification, Verification::Md5(_)));
    assert!(gog.file_requests().is_empty());
    assert_eq!(gog.names(), [".en1installer0.json", SETUP]);
}

#[test]
fn a_second_downloader_waits_for_the_first() {
    let gog = Gog::one();
    let held = gog
        .cache
        .lock(PRODUCT, &|| false, Endpoint::Download)
        .unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        let gog = &gog;
        scope.spawn(move || sender.send(gog.get(&gog.package("en1installer0"))).unwrap());
        assert!(receiver.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(gog.server.requests().is_empty());
        drop(held);
        let downloaded = receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(
            fs::read(&downloaded.path).unwrap(),
            gog.body("en1installer0")
        );
    });
}

#[test]
fn a_downloader_waiting_while_the_product_is_removed_locks_it_again() {
    let gog = Gog::one();
    let held = gog.cache.lock(PRODUCT, &|| false, Endpoint::Cache).unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        let gog = &gog;
        scope.spawn(move || sender.send(gog.get(&gog.package("en1installer0"))).unwrap());
        assert!(receiver.recv_timeout(Duration::from_millis(300)).is_err());
        // wineo never removes the lock file, but someone clearing the cache
        // by hand might, while the downloader waits on it.
        fs::remove_dir_all(gog.cache.dir().join(PRODUCT.to_string())).unwrap();
        drop(held);
        let downloaded = receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert!(downloaded.path.exists());
        assert!(
            gog.cache
                .dir()
                .join(PRODUCT.to_string())
                .join(".lock")
                .exists()
        );
    });
    drop(gog.cache.lock(PRODUCT, &|| false, Endpoint::Cache).unwrap());
    gog.cache.remove_product(PRODUCT, &|| false).unwrap();
    assert_eq!(gog.cache.files(PRODUCT).unwrap(), []);
}

#[test]
fn removing_a_product_keeps_its_lock_so_newcomers_wait() {
    let gog = Gog::start(&[
        ("en1installer0", SETUP, body(1000, 1)),
        ("en1installer1", PART1, body(2000, 2)),
    ]);
    gog.get(&gog.package("en1installer0")).unwrap();
    gog.get(&gog.package("en1installer1")).unwrap();
    // Another download folder, so there's more than one entry to remove.
    let product = gog.cache.dir().join(PRODUCT.to_string());
    fs::create_dir_all(product.join("patch_x")).unwrap();
    let lock = product.join(".lock");
    let inode = |path: &PathBuf| fs::metadata(path).unwrap().ino();
    let before = inode(&lock);
    let steps = Cell::new(0);
    gog.cache
        .remove_product_with(PRODUCT, &|| false, &|| {
            steps.set(steps.get() + 1);
            // The same lock file is there, and still held, at every step.
            assert_eq!(inode(&lock), before);
            let file = fs::File::open(&lock).unwrap();
            assert_eq!(
                flock(&file, FlockOperation::NonBlockingLockExclusive),
                Err(Errno::WOULDBLOCK)
            );
            // A newcomer arriving now waits rather than locking a new file.
            let error = gog
                .cache
                .lock(PRODUCT, &|| true, Endpoint::Download)
                .err()
                .unwrap();
            assert_eq!(error.kind(), ErrorKind::Canceled);
        })
        .unwrap();
    assert_eq!(steps.get(), 2);
    assert_eq!(gog.product_entries(), [".lock"]);
    assert_eq!(inode(&lock), before);
}

#[test]
fn removing_a_product_waits_for_its_download() {
    let gog = Gog::one();
    gog.get(&gog.package("en1installer0")).unwrap();
    let held = gog
        .cache
        .lock(PRODUCT, &|| false, Endpoint::Download)
        .unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        let cache = &gog.cache;
        scope.spawn(move || {
            sender
                .send(cache.remove_product(PRODUCT, &|| false))
                .unwrap()
        });
        assert!(receiver.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(gog.folder().join(SETUP).exists());
        drop(held);
        receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
    });
    assert_eq!(gog.product_entries(), [".lock"]);
}

#[test]
fn a_pin_holds_a_products_files_until_it_removes_them_or_lets_go() {
    let gog = Gog::one();
    gog.get(&gog.package("en1installer0")).unwrap();
    let pin = gog.cache.pin(PRODUCT, &|| false).unwrap();
    assert!(
        pin.complete(&gog.package("en1installer0"), None, &|| false)
            .unwrap()
            .is_some()
    );
    // Nobody else removes or downloads meanwhile.
    let error = gog.cache.remove_product(PRODUCT, &|| true).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Canceled);
    assert!(gog.folder().join(SETUP).exists());
    drop(pin);
    assert!(gog.folder().join(SETUP).exists());

    let pin = gog.cache.pin(PRODUCT, &|| false).unwrap();
    pin.remove().unwrap();
    assert_eq!(gog.product_entries(), [".lock"]);
    // Removing let go of the lock.
    drop(gog.cache.pin(PRODUCT, &|| true).unwrap());
}

#[test]
fn canceling_a_wait_for_the_lock_leaves_the_holder_alone() {
    let gog = Gog::one();
    let first = gog
        .cache
        .lock(PRODUCT, &|| false, Endpoint::Download)
        .unwrap();
    let canceled = AtomicBool::new(false);
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        let (gog, canceled) = (&gog, &canceled);
        scope.spawn(move || {
            sender
                .send(gog.get_with(
                    &gog.package("en1installer0"),
                    &|| canceled.load(Ordering::Relaxed),
                    &|_, _| {},
                ))
                .unwrap()
        });
        assert!(receiver.recv_timeout(Duration::from_millis(300)).is_err());
        canceled.store(true, Ordering::Relaxed);
        let error = receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind()),
            (Endpoint::Download, ErrorKind::Canceled)
        );
        assert!(gog.server.requests().is_empty());

        // A removal waiting on the lock can be canceled the same way.
        let (sender, receiver) = mpsc::channel();
        scope.spawn(move || {
            sender
                .send(
                    gog.cache
                        .remove_product(PRODUCT, &|| canceled.load(Ordering::Relaxed)),
                )
                .unwrap()
        });
        let error = receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind()),
            (Endpoint::Cache, ErrorKind::Canceled)
        );
    });

    // The holder still has the lock: nobody else can take it.
    let other = gog.cache.dir().join(PRODUCT.to_string()).join(".lock");
    let file = fs::File::open(&other).unwrap();
    assert_eq!(
        flock(&file, FlockOperation::NonBlockingLockExclusive),
        Err(Errno::WOULDBLOCK)
    );
    drop(first);
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert_eq!(
        fs::read(&downloaded.path).unwrap(),
        gog.body("en1installer0")
    );
}

#[test]
fn a_range_the_server_can_t_satisfy_starts_over_from_zero() {
    let gog = Gog::one();
    gog.set(|script| script.ranges = Ranges::Unsatisfiable);
    let body = gog.body("en1installer0");
    gog.leave_partial(gog.record("en1installer0"), &body[..1000]);
    let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
    assert_eq!(fs::read(&downloaded.path).unwrap(), body);
    let files = gog.file_requests();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].header("range"), Some("bytes=1000-"));
    assert_eq!(files[1].header("range"), None);
}

#[test]
fn a_server_that_keeps_answering_416_fails_without_a_partial() {
    let gog = Gog::one();
    gog.set(|script| script.ranges = Ranges::AlwaysUnsatisfiable);
    let body = gog.body("en1installer0");
    gog.leave_partial(gog.record("en1installer0"), &body[..1000]);
    let error = gog.get(&gog.package("en1installer0")).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Download, ErrorKind::WrongRange)
    );
    assert_eq!(gog.file_requests().len(), 2);
    assert!(gog.names().is_empty(), "{:?}", gog.names());
}

#[test]
fn a_renumbered_file_reusing_a_name_replaces_the_stale_one() {
    // GOG now lists the setup program as en1installer1.
    let gog = Gog::start(&[("en1installer1", SETUP, body(1000, 1))]);
    let stale = Record {
        file_id: "en1installer0".to_owned(),
        name: SETUP.to_owned(),
        size: 900,
        md5: None,
    };
    gog.leave_partial(stale, &[7; 100]);
    fs::write(gog.folder().join(SETUP), [7; 900]).unwrap();

    let downloaded = gog.get(&gog.package("en1installer1")).unwrap();
    assert_eq!(
        fs::read(&downloaded.path).unwrap(),
        gog.body("en1installer1")
    );
    assert_eq!(gog.names(), [".en1installer1.json", SETUP]);
}

#[test]
fn a_name_clash_with_no_file_list_is_refused() {
    let gog = Gog::start(&[
        ("en1installer0", SETUP, body(1000, 1)),
        ("en1installer9", SETUP, body(2000, 2)),
    ]);
    gog.get(&gog.package("en1installer0")).unwrap();
    let unknown = Package {
        siblings: &[],
        ..gog.package("en1installer9")
    };
    let error = gog.get(&unknown).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Malformed);
    assert!(gog.folder().join(".en1installer0.json").exists());
}
