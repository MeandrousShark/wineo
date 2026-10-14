//! Local HTTP servers standing in for GOG. A handler answers each request,
//! and every request is recorded.

use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Url;

use crate::client::{Client, Endpoints};

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// The path and query, as sent.
    pub target: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }

    /// A decoded query parameter.
    pub fn query(&self, name: &str) -> Option<String> {
        let url = Url::parse(&format!("http://fake{}", self.target)).ok()?;
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub enum Reply {
    Send(Vec<u8>),
    /// Writes the bytes, then holds the connection open, so a short client
    /// timeout runs out.
    Stall(Vec<u8>, Duration),
}

pub fn json(status: u16, body: &str) -> Reply {
    Reply::Send(raw(status, body))
}

pub fn raw(status: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// A response with any headers and a binary body.
pub fn response(status: u16, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status} X\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// A response with no `Content-Length`, its end marked by closing the
/// connection.
pub fn unsized_response(status: u16, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status} X\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// Serves a file the way a download server does: whole, or from an offset
/// with `Range: bytes=N-`.
pub fn file(request: &Request, body: &[u8]) -> Reply {
    let start = request
        .header("range")
        .and_then(|range| range.strip_prefix("bytes="))
        .and_then(|range| range.strip_suffix('-'))
        .and_then(|start| start.parse::<usize>().ok());
    Reply::Send(match start {
        None => response(200, &[], body),
        Some(start) if start < body.len() => response(
            206,
            &[(
                "Content-Range",
                format!("bytes {start}-{}/{}", body.len() - 1, body.len()),
            )],
            &body[start..],
        ),
        Some(_) => response(
            416,
            &[("Content-Range", format!("bytes */{}", body.len()))],
            &[],
        ),
    })
}

pub struct FakeGog {
    pub base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
}

impl FakeGog {
    pub fn start(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);
        let (seen, stopped) = (Arc::clone(&requests), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => return,
                };
                let (seen, handler) = (Arc::clone(&seen), Arc::clone(&handler));
                std::thread::spawn(move || {
                    stream.set_nonblocking(false).unwrap();
                    let Some(request) = read_request(&mut stream) else {
                        return;
                    };
                    seen.lock().unwrap().push(request.clone());
                    let (bytes, stall) = match handler(&request) {
                        Reply::Send(bytes) => (bytes, Duration::ZERO),
                        Reply::Stall(bytes, stall) => (bytes, stall),
                    };
                    let _ = stream.write_all(&bytes);
                    let _ = stream.flush();
                    std::thread::sleep(stall);
                });
            }
        });
        Self {
            base,
            requests,
            stop,
        }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    pub fn client(&self) -> Client {
        Client::with_base("wineo-test/1", &self.base).unwrap()
    }

    /// A client whose reads give up quickly, for timeouts.
    pub fn quick_client(&self) -> Client {
        Client::build(
            "wineo-test/1",
            Endpoints::all_at(&self.base).unwrap(),
            false,
            Duration::from_millis(500),
            Duration::from_millis(150),
        )
        .unwrap()
    }
}

impl Drop for FakeGog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A server that answers each request line through a handler, one request
/// at a time. The handler runs while the client waits for its answer, so a
/// test can change state at exactly that point.
pub struct Scripted {
    pub base: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Scripted {
    pub fn start(handler: impl Fn(&str) -> Vec<u8> + Send + 'static) -> Self {
        Self::start_paced(handler, usize::MAX, Duration::ZERO)
    }

    /// `start`, writing each answer `piece_size` bytes at a time with
    /// `pause` between them, like a slow connection.
    pub fn start_paced(
        handler: impl Fn(&str) -> Vec<u8> + Send + 'static,
        piece_size: usize,
        pause: Duration,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopped) = (Arc::clone(&requests), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => return,
                };
                stream.set_nonblocking(false).unwrap();
                let mut head = Vec::new();
                let mut chunk = [0; 1024];
                while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => head.extend_from_slice(&chunk[..read]),
                    }
                }
                let line = String::from_utf8_lossy(&head)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                seen.lock().unwrap().push(line.clone());
                for piece in handler(&line).chunks(piece_size) {
                    if stream.write_all(piece).is_err() {
                        break;
                    }
                    let _ = stream.flush();
                    std::thread::sleep(pause);
                }
                let _ = stream.flush();
            }
        });
        Self {
            base,
            requests,
            stop,
        }
    }

    /// The request lines received so far.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    pub fn client(&self) -> Client {
        Client::with_base("wineo-test", &self.base).unwrap()
    }
}

impl Drop for Scripted {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<Request> {
    let mut head = Vec::new();
    let mut chunk = [0; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(read) => head.extend_from_slice(&chunk[..read]),
        }
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    let headers = lines
        .take_while(|line| !line.is_empty())
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    Some(Request {
        method,
        target,
        headers,
    })
}

/// A base URL nothing listens on.
pub fn refused() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", listener.local_addr().unwrap())
}
