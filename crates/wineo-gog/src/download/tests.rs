use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::cache::{CachedFile, write_record};
use crate::error::Class;
use crate::fake::{self, FakeGog, Reply, Request, json as reply, response};

const ACCESS: &str = "ACCESSSECRET0123456789";
const PRODUCT: u64 = 1094900565;
const DOWNLOAD: &str = "installer_windows_en";
const SETUP: &str = "setup_game_(64bit)_(123).exe";
const PART1: &str = "setup_game_(64bit)_(123)-1.bin";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sum {
    Right,
    Wrong,
    NoUrl,
    Missing,
    Junk,
    WrongTotal,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ranges {
    Honor,
    Whole,
    WrongStart,
    WrongTotal,
    /// A 206 with a range that never fits, even without a Range request.
    AlwaysWrong,
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
    resolve_status: Option<u16>,
    file_status: Option<u16>,
    /// Links go through a redirect to the real file.
    redirect: bool,
}

struct Gog {
    server: FakeGog,
    script: Arc<Mutex<Script>>,
    _dir: TempDir,
    cache: DownloadCache,
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
        let location = format!("http://{host}/cdn/{rest}?token=SIGNED");
        return Reply::Send(response(302, &[("Location", location)], &[]));
    }
    let Some(rest) = path.strip_prefix("/cdn/") else {
        return reply(404, "{}");
    };
    let (n, segment) = rest.split_once('/').unwrap();
    let n: u32 = n.parse().unwrap();
    if script.refuse_always || n <= script.expired_through {
        let status = script.refuse_with;
        return Reply::Send(response(status, &[], b"expired SIGNED"));
    }
    if let Some(segment) = segment.strip_suffix(".xml") {
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
            _ => (md5_of(body), body.len()),
        };
        return match script.sum {
            Sum::Missing => reply(404, ""),
            Sum::Junk => Reply::Send(response(200, &[], b"<html>no</html>")),
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
            resolve_status: None,
            file_status: None,
            redirect: false,
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
        }
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

    fn package<'a>(&self, file_id: &'a str) -> Package<'a> {
        Package {
            product_id: PRODUCT,
            download_id: DOWNLOAD,
            file_id,
            size: self.body(file_id).len() as u64,
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
    for status in [401, 403] {
        let gog = Gog::one();
        gog.set(|script| {
            script.expired_through = 1;
            script.refuse_with = status;
        });
        let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
        assert_eq!(
            fs::read(&downloaded.path).unwrap(),
            gog.body("en1installer0")
        );
        // The first link's checksum was refused too; the second one's wasn't.
        assert!(matches!(downloaded.verification, Verification::Md5(_)));
        assert_eq!(gog.script.lock().unwrap().resolves, 2);
        let files = gog.file_requests();
        assert_eq!(files.len(), 2);
        assert!(files[1].path().starts_with("/cdn/2/"));
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
fn without_a_checksum_only_the_size_is_checked() {
    for sum in [Sum::NoUrl, Sum::Missing, Sum::Junk] {
        let gog = Gog::one();
        gog.set(|script| script.sum = sum);
        let downloaded = gog.get(&gog.package("en1installer0")).unwrap();
        assert_eq!(downloaded.verification, Verification::SizeOnly, "{sum:?}");
        assert_eq!(
            fs::read(&downloaded.path).unwrap(),
            gog.body("en1installer0")
        );
    }
}

#[test]
fn a_file_that_isn_t_the_listed_size_fails() {
    let gog = Gog::one();
    let size = gog.body("en1installer0").len() as u64;
    let bigger = Package {
        size: size + 10,
        ..gog.package("en1installer0")
    };
    let smaller = Package {
        size: size - 10,
        ..gog.package("en1installer0")
    };
    for (sum, package) in [
        (Sum::Right, bigger),
        (Sum::NoUrl, bigger),
        (Sum::NoUrl, smaller),
        (Sum::WrongTotal, gog.package("en1installer0")),
    ] {
        gog.set(|script| script.sum = sum);
        let error = gog.get(&package).unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind()),
            (Endpoint::Download, ErrorKind::WrongSize),
            "{sum:?} {}",
            package.size
        );
        assert_clean(&error);
        assert!(!gog.folder().join(SETUP).exists());
    }
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
    assert_eq!(seen.get(), (package.size, package.size));
    assert_eq!(gog.server.requests().len(), requests);

    let md5 = md5_of(&gog.body("en1installer0"));
    let complete = |md5: Option<&str>| gog.cache.complete(&package, md5, &|| false).unwrap();
    assert_eq!(complete(None), Some(first.path.clone()));
    assert_eq!(
        complete(Some(&md5.to_ascii_uppercase())),
        Some(first.path.clone())
    );
    assert_eq!(complete(Some(&md5_of(b"other"))), None);
    let other_size = Package {
        size: package.size - 1,
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

    gog.cache.remove_product(PRODUCT).unwrap();
    assert_eq!(gog.cache.files(PRODUCT).unwrap(), []);
    assert!(!gog.cache.dir().join(PRODUCT.to_string()).exists());
    gog.cache.remove_product(PRODUCT).unwrap();
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
    let setups: [fn(&mut Script); 8] = [
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
    let held = gog.cache.lock(PRODUCT, Endpoint::Download).unwrap();
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
    let held = gog.cache.lock(PRODUCT, Endpoint::Cache).unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        let gog = &gog;
        scope.spawn(move || sender.send(gog.get(&gog.package("en1installer0"))).unwrap());
        assert!(receiver.recv_timeout(Duration::from_millis(300)).is_err());
        // What remove_product does while holding the lock: the lock file
        // the downloader waits on goes with the product.
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
    // A lock taken now is the same one the downloader used, not a new file.
    drop(gog.cache.lock(PRODUCT, Endpoint::Cache).unwrap());
    gog.cache.remove_product(PRODUCT).unwrap();
    assert_eq!(gog.cache.files(PRODUCT).unwrap(), []);
}

#[test]
fn removing_a_product_waits_for_its_download() {
    let gog = Gog::one();
    gog.get(&gog.package("en1installer0")).unwrap();
    let held = gog.cache.lock(PRODUCT, Endpoint::Download).unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        let cache = &gog.cache;
        scope.spawn(move || sender.send(cache.remove_product(PRODUCT)).unwrap());
        assert!(receiver.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(gog.folder().join(SETUP).exists());
        drop(held);
        receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
    });
    assert!(!gog.cache.dir().join(PRODUCT.to_string()).exists());
}
