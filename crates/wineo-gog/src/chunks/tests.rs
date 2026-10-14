use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::error::Class;
use crate::fake::content::{Cdn, ContentSystem, FakeBuild, FakeDepot, FakeFile, md5_hex, zlib};
use crate::fake::{FakeGog, Reply, Request, json as reply, response};
use crate::manifest::DepotItem;

const ACCESS: &str = "ACCESSSECRET0123456789";
const MD5: &str = "0123456789abcdef0123456789abcdef";

fn endpoint(format: &str, parameters: &[(&str, &str)]) -> LinkEndpoint {
    LinkEndpoint {
        name: "cdn".to_owned(),
        url_format: format.to_owned(),
        parameters: parameters
            .iter()
            .map(|&(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
        priority: 0,
        max_fails: None,
        fallback_only: false,
    }
}

#[test]
fn a_chunk_url_appends_the_hash_to_the_path_and_fills_the_template() {
    let fastly = endpoint(
        "{base_url}/token=nva={expires_at}~dirs={dirs}~token={token}{path}",
        &[
            ("base_url", "https://gog-cdn-fastly.gog.com"),
            ("expires_at", "1792088159"),
            ("dirs", "4"),
            ("token", "TOK"),
            ("path", "/content-system/v2/store/1207658924/"),
        ],
    );
    assert_eq!(
        fastly.chunk_url(MD5).unwrap().as_str(),
        format!(
            "https://gog-cdn-fastly.gog.com/token=nva=1792088159~dirs=4~token=TOK/content-system/v2/store/1207658924/01/23/{MD5}"
        )
    );
    let gcore = endpoint(
        "{base_url}{path}?wsSecret={token}&wsTime={time}&x={unknown}",
        &[
            ("base_url", "https://gog-cdn.gcdn.co"),
            ("path", "/store/1"),
            // A value that looks like a key is used as it is.
            ("token", "{time}"),
            ("time", "5"),
        ],
    );
    assert_eq!(
        gcore.chunk_url(MD5).unwrap().as_str(),
        format!(
            "https://gog-cdn.gcdn.co/store/1/01/23/{MD5}?wsSecret={{time}}&wsTime=5&x={{unknown}}"
        )
    );
    assert!(
        endpoint("{base_url}", &[("base_url", "https://a")])
            .chunk_url(MD5)
            .is_none()
    );
    assert!(
        endpoint("{path}", &[("path", "/x")])
            .chunk_url(MD5)
            .is_none()
    );
    assert!(fastly.chunk_url("ab").is_none());
}

fn link_json() -> Value {
    json!({
        "product_id": "1",
        "type": "depot",
        "urls": [
            {"endpoint_name": "backup", "url_format": "{base_url}{path}", "parameters": {"base_url": "https://b", "path": "/"}, "priority": 99, "fallback_only": true, "max_fails": 100},
            {"endpoint_name": "low", "url_format": "{base_url}{path}", "parameters": {"base_url": "https://l", "path": "/", "expires_at": 2000}, "priority": 1, "fallback_only": false},
            {"endpoint_name": "high", "url_format": "{base_url}{path}", "parameters": {"base_url": "https://h", "path": "/", "expires_at": "1000"}, "priority": 10, "fallback_only": false},
            {"endpoint_name": "same", "url_format": "{base_url}{path}", "parameters": {"base_url": "https://s", "path": "/"}, "priority": 1},
        ],
    })
}

#[test]
fn endpoints_go_by_priority_with_fallbacks_last() {
    let link = parse_secure_link(&link_json()).unwrap();
    assert_eq!(link.endpoint_names(), ["high", "low", "same", "backup"]);
    assert_eq!(link.expires_at(), Some(1000));
}

#[test]
fn a_link_is_reused_until_a_minute_before_it_expires() {
    let link = parse_secure_link(&link_json()).unwrap();
    assert!(link.fresh(0));
    assert!(link.fresh(939));
    assert!(!link.fresh(940));
    assert!(!link.fresh(5000));
    let mut forever = link_json();
    for endpoint in forever["urls"].as_array_mut().unwrap() {
        endpoint["parameters"]
            .as_object_mut()
            .unwrap()
            .remove("expires_at");
    }
    let forever = parse_secure_link(&forever).unwrap();
    assert_eq!(forever.expires_at(), None);
    assert!(forever.fresh(u64::MAX));
}

#[test]
fn a_secure_link_without_usable_endpoints_is_refused() {
    for broken in [
        json!({}),
        json!({"urls": []}),
        json!({"urls": [{"endpoint_name": "a", "parameters": {}}]}),
        json!({"urls": [{"url_format": "x", "parameters": {}}]}),
        json!({"urls": [{"endpoint_name": "a", "url_format": "x"}]}),
        json!({"urls": [{"endpoint_name": "a", "url_format": "x", "parameters": {"k": [1]}}]}),
    ] {
        assert!(parse_secure_link(&broken).is_none(), "{broken}");
    }
}

#[test]
fn debug_output_never_shows_a_link() {
    let link = parse_secure_link(&json!({
        "urls": [{
            "endpoint_name": "fastly",
            "url_format": "https://cdn.example/token=SECRETTOKEN{path}",
            "parameters": {"token": "SECRETTOKEN", "path": "/store", "expires_at": 5},
            "priority": 10,
        }],
    }))
    .unwrap();
    let links = ChunkLinks::new(LinkScope::Product(1));
    *links.current.lock().unwrap() = (1, Some(Arc::new(link.clone())));
    let shown = format!("{link:?} {link:#?} {links:?}");
    for secret in ["SECRETTOKEN", "cdn.example", "/store", "token="] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
    assert!(shown.contains("fastly") && shown.contains("held: true"));
}

/// A content system with one product of two files, the first in three
/// chunks with both whole-file hashes, behind a fake GOG.
fn serve(content: ContentSystem) -> (Arc<ContentSystem>, FakeGog, Vec<DepotFile>) {
    content.add_build(
        FakeBuild::new(7, "100").depot(
            FakeDepot::new(7)
                .file(
                    FakeFile::new("bin\\game.exe", b"0123456789")
                        .with_md5()
                        .with_sha256(),
                )
                .file(FakeFile::new("empty.txt", b"")),
        ),
    );
    let content = Arc::new(content);
    let served = Arc::clone(&content);
    let server =
        FakeGog::start(move |request| served.answer(request).unwrap_or_else(|| reply(404, "{}")));
    let client = server.client();
    let builds = client.windows_builds(7, None).unwrap();
    let repository = client
        .repository(builds.items[0].link.as_deref().unwrap())
        .unwrap();
    let files = client
        .depot_manifest(&repository.depots[0].manifest)
        .unwrap()
        .items
        .into_iter()
        .filter_map(|item| match item {
            DepotItem::File(file) => Some(file),
            _ => None,
        })
        .collect();
    (content, server, files)
}

fn link_requests(server: &FakeGog) -> Vec<Request> {
    server
        .requests()
        .into_iter()
        .filter(|request| {
            request.path().ends_with("/secure_link") || request.path() == "/open_link"
        })
        .collect()
}

fn chunk_requests(server: &FakeGog) -> Vec<Cdn> {
    server
        .requests()
        .iter()
        .filter_map(ContentSystem::cdn_of)
        .collect()
}

/// Every way an error could be shown, which must hold no link, token, or
/// address.
fn assert_clean(error: &Error, content: &ContentSystem) {
    let shown = format!("{error} {error:?} {error:#?} {}", error.kind());
    let mut secrets = vec![
        ACCESS.to_owned(),
        "FAKETOKEN".to_owned(),
        "wsSecret".to_owned(),
        "nva=".to_owned(),
        "127.0.0.1".to_owned(),
        "?".to_owned(),
        "/store".to_owned(),
    ];
    secrets.extend(content.live_tokens());
    for secret in secrets {
        assert!(!shown.contains(&secret), "{secret} in {shown}");
    }
}

#[test]
fn a_file_is_assembled_from_its_chunks_and_checked_whole() {
    let (_content, server, files) = serve(ContentSystem::new().with_chunk_size(4));
    let client = server.client();
    let links = ChunkLinks::new(LinkScope::Product(7));
    assert_eq!(files[0].chunks.len(), 3);
    assert!(files[0].md5.is_some() && files[0].sha256.is_some());
    let mut out = Vec::new();
    client
        .write_file(&links, ACCESS, &files[0], &mut out, &|| false)
        .unwrap();
    assert_eq!(out, b"0123456789");
    let mut empty = Vec::new();
    client
        .write_file(&links, ACCESS, &files[1], &mut empty, &|| false)
        .unwrap();
    assert!(empty.is_empty());

    // One link for every chunk, fetched with the token and `path` as is.
    let [request] = link_requests(&server).try_into().unwrap();
    assert_eq!(
        request.target,
        "/products/7/secure_link?_version=2&generation=2&path=/"
    );
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {ACCESS}").as_str())
    );
    // The primary CDN, never the fallback that claims a higher priority.
    assert_eq!(chunk_requests(&server), [Cdn::Primary; 3]);
    for request in server.requests() {
        if ContentSystem::cdn_of(&request).is_some() {
            assert_eq!(request.header("authorization"), None);
        }
    }
}

#[test]
fn a_wrong_whole_file_checksum_fails_after_writing() {
    let (content, server, files) = serve(ContentSystem::new().with_chunk_size(4));
    let client = server.client();
    let links = ChunkLinks::new(LinkScope::Product(7));
    for (md5, sha256) in [
        (Some(md5_hex(b"other")), files[0].sha256.clone()),
        (files[0].md5.clone(), Some("0".repeat(64))),
    ] {
        let file = DepotFile {
            md5,
            sha256,
            ..files[0].clone()
        };
        let error = client
            .write_file(&links, ACCESS, &file, &mut Vec::new(), &|| false)
            .unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind()),
            (Endpoint::Chunk, ErrorKind::WrongChecksum)
        );
        assert_clean(&error, &content);
    }
}

#[test]
fn a_canceled_or_unwritable_file_stops() {
    let (content, server, files) = serve(ContentSystem::new().with_chunk_size(4));
    let client = server.client();
    let links = ChunkLinks::new(LinkScope::Product(7));
    let error = client
        .write_file(&links, ACCESS, &files[0], &mut Vec::new(), &|| true)
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Canceled);
    assert!(chunk_requests(&server).is_empty());

    struct Full;
    impl Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::StorageFull.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let error = client
        .write_file(&links, ACCESS, &files[0], &mut Full, &|| false)
        .unwrap_err();
    assert_eq!(
        error.kind(),
        ErrorKind::Disk(std::io::ErrorKind::StorageFull)
    );
    assert_clean(&error, &content);
}

#[test]
fn a_link_about_to_expire_is_fetched_again() {
    let content = ContentSystem::new().with_chunk_size(4);
    content.set_link_lifetime(30);
    let (_content, server, files) = serve(content);
    let client = server.client();
    let links = ChunkLinks::new(LinkScope::Product(7));
    for chunk in &files[0].chunks {
        client.chunk(&links, ACCESS, chunk).unwrap();
    }
    assert_eq!(link_requests(&server).len(), 3);
}

#[test]
fn a_refused_link_is_fetched_again_once() {
    for status in [401, 403] {
        let (content, server, files) = serve(ContentSystem::new().with_chunk_size(4));
        let client = server.client();
        let links = ChunkLinks::new(LinkScope::Product(7));
        let chunk = &files[0].chunks[0];
        client.chunk(&links, ACCESS, chunk).unwrap();
        content.revoke_links(status);
        assert_eq!(client.chunk(&links, ACCESS, chunk).unwrap(), b"0123");
        assert_eq!(link_requests(&server).len(), 2);
        // Refused, then the new link on the primary again, never the
        // fallback.
        assert_eq!(chunk_requests(&server), [Cdn::Primary; 3]);
    }
}

#[test]
fn a_link_refused_even_when_new_fails_without_naming_it() {
    for status in [401, 403] {
        let content = Arc::new(ContentSystem::new());
        let chunk = content.add_chunk(b"data");
        let served = Arc::clone(&content);
        let server = FakeGog::start(move |request| {
            if ContentSystem::cdn_of(request).is_some() {
                return reply(status, "{}");
            }
            served.answer(request).unwrap_or_else(|| reply(404, "{}"))
        });
        let links = ChunkLinks::new(LinkScope::Product(7));
        let error = server.client().chunk(&links, ACCESS, &chunk).unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind(), error.class()),
            (Endpoint::Chunk, ErrorKind::Status(status), Class::Other)
        );
        assert_clean(&error, &content);
        assert_eq!(link_requests(&server).len(), 2);
        // The first link's primary, then the new link's primary and
        // fallback.
        assert_eq!(
            chunk_requests(&server),
            [Cdn::Primary, Cdn::Primary, Cdn::Fallback]
        );
    }
}

#[test]
fn a_chunk_missing_on_the_primary_comes_from_the_fallback() {
    let (content, server, files) = serve(ContentSystem::new().with_chunk_size(4));
    let chunk = &files[0].chunks[1];
    content.missing_on_primary(&chunk.compressed_md5);
    let links = ChunkLinks::new(LinkScope::Product(7));
    assert_eq!(
        server.client().chunk(&links, ACCESS, chunk).unwrap(),
        b"4567"
    );
    assert_eq!(chunk_requests(&server), [Cdn::Primary, Cdn::Fallback]);
    let fallback = server.requests().pop().unwrap();
    assert!(
        fallback
            .query("wsSecret")
            .is_some_and(|token| token.starts_with("FAKETOKEN"))
    );
    assert!(fallback.path().ends_with(&format!(
        "/content-system/v2/store/7/{}/{}/{}",
        &chunk.compressed_md5[0..2],
        &chunk.compressed_md5[2..4],
        chunk.compressed_md5
    )));
}

#[test]
fn a_chunk_failing_its_compressed_checksum_is_refused() {
    let (content, server, files) = serve(ContentSystem::new().with_chunk_size(4));
    let chunk = &files[0].chunks[0];
    content.corrupt_chunk(&chunk.compressed_md5);
    let links = ChunkLinks::new(LinkScope::Product(7));
    let error = server.client().chunk(&links, ACCESS, chunk).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind(), error.class()),
        (Endpoint::Chunk, ErrorKind::WrongChecksum, Class::Other)
    );
    assert_eq!(chunk_requests(&server), [Cdn::Primary, Cdn::Fallback]);
    assert_clean(&error, &content);
}

#[test]
fn a_chunk_failing_its_unpacked_checksum_or_size_is_refused() {
    let (content, server, files) = serve(ContentSystem::new().with_chunk_size(4));
    let client = server.client();
    let links = ChunkLinks::new(LinkScope::Product(7));
    let good = &files[0].chunks[0];
    for (chunk, kind) in [
        (
            Chunk {
                md5: md5_hex(b"other"),
                ..good.clone()
            },
            ErrorKind::WrongChecksum,
        ),
        (
            Chunk {
                size: good.size + 1,
                ..good.clone()
            },
            ErrorKind::WrongSize,
        ),
        (
            Chunk {
                size: good.size - 1,
                ..good.clone()
            },
            ErrorKind::WrongSize,
        ),
        (
            Chunk {
                compressed_size: good.compressed_size - 1,
                ..good.clone()
            },
            ErrorKind::WrongSize,
        ),
    ] {
        let error = client.chunk(&links, ACCESS, &chunk).unwrap_err();
        assert_eq!((error.endpoint(), error.kind()), (Endpoint::Chunk, kind));
        assert_clean(&error, &content);
    }
}

#[test]
fn a_chunk_that_is_not_zlib_is_malformed() {
    let content = Arc::new(ContentSystem::new());
    let garbage = b"not zlib at all".to_vec();
    let chunk = Chunk {
        md5: md5_hex(b"x"),
        size: 1,
        compressed_md5: md5_hex(&garbage),
        compressed_size: garbage.len() as u64,
    };
    let served = Arc::clone(&content);
    let server = FakeGog::start(move |request| {
        if ContentSystem::cdn_of(request).is_some() {
            return Reply::Send(response(200, &[], &garbage));
        }
        served.answer(request).unwrap_or_else(|| reply(404, "{}"))
    });
    let links = ChunkLinks::new(LinkScope::Product(7));
    let error = server.client().chunk(&links, ACCESS, &chunk).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Malformed);
    assert_clean(&error, &content);
}

#[test]
fn a_chunk_missing_everywhere_is_a_404() {
    let (content, server, _files) = serve(ContentSystem::new());
    let compressed = zlib(b"never served");
    let chunk = Chunk {
        md5: md5_hex(b"never served"),
        size: 12,
        compressed_md5: md5_hex(&compressed),
        compressed_size: compressed.len() as u64,
    };
    let links = ChunkLinks::new(LinkScope::Product(7));
    let error = server.client().chunk(&links, ACCESS, &chunk).unwrap_err();
    assert_eq!(
        (error.endpoint(), error.kind()),
        (Endpoint::Chunk, ErrorKind::Status(404))
    );
    assert_clean(&error, &content);
}

#[test]
fn the_dependency_store_has_its_own_link() {
    let content = ContentSystem::new();
    let (content, server, _files) = serve(content);
    let chunk = content.add_chunk(b"redist bytes");
    let links = ChunkLinks::new(LinkScope::Dependencies);
    assert_eq!(links.scope(), LinkScope::Dependencies);
    assert_eq!(
        server.client().chunk(&links, ACCESS, &chunk).unwrap(),
        b"redist bytes"
    );
    let [request] = link_requests(&server).try_into().unwrap();
    assert_eq!(
        request.target,
        "/open_link?generation=2&_version=2&path=/dependencies/store/"
    );
    let chunk_request = server.requests().pop().unwrap();
    assert!(
        chunk_request
            .path()
            .contains("/content-system/v2/dependencies/store/")
    );
}

#[test]
fn secure_link_failures_are_classified_without_the_token() {
    for (status, kind) in [
        (401, ErrorKind::Unauthorized),
        (403, ErrorKind::Status(403)),
        (500, ErrorKind::Server(500)),
    ] {
        let server = FakeGog::start(move |_| reply(status, &format!("{{\"echo\":\"{ACCESS}\"}}")));
        let client = server.client();
        let links = ChunkLinks::new(LinkScope::Product(7));
        let chunk = Chunk {
            md5: MD5.to_owned(),
            size: 1,
            compressed_md5: MD5.to_owned(),
            compressed_size: 1,
        };
        for error in [
            client.chunk(&links, ACCESS, &chunk).unwrap_err(),
            client
                .secure_link(ACCESS, LinkScope::Dependencies)
                .unwrap_err(),
        ] {
            assert_eq!(
                (error.endpoint(), error.kind()),
                (Endpoint::SecureLink, kind)
            );
            assert_clean(&error, &ContentSystem::new());
        }
    }
}

#[test]
fn a_secure_link_for_another_product_or_without_urls_is_malformed() {
    for body in [
        json!({"product_id": "8", "urls": link_json()["urls"]}),
        json!({"product_id": "7", "urls": []}),
        json!({"product_id": "7"}),
    ] {
        let server = FakeGog::start(move |_| reply(200, &body.to_string()));
        let error = server
            .client()
            .secure_link(ACCESS, LinkScope::Product(7))
            .unwrap_err();
        assert_eq!(
            (error.endpoint(), error.kind(), error.class()),
            (Endpoint::SecureLink, ErrorKind::Malformed, Class::Malformed)
        );
    }
}
