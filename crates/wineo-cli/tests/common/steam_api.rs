//! A local HTTP server standing in for Steam's Web API. Each connection gets
//! the next canned reply, and the request lines it saw are kept.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct SteamApi {
    pub base: String,
    replies: Arc<Mutex<VecDeque<String>>>,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// A 200 answer with a JSON body.
pub fn json(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

pub fn status(code: u16) -> String {
    format!("HTTP/1.1 {code} Reply\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

/// The answer for a library of `(app id, name)` pairs.
pub fn owned(games: &[(u64, &str)]) -> String {
    let list: Vec<String> = games
        .iter()
        .map(|(id, name)| format!("{{\"appid\":{id},\"name\":\"{name}\"}}"))
        .collect();
    json(&format!(
        "{{\"response\":{{\"game_count\":{},\"games\":[{}]}}}}",
        games.len(),
        list.join(",")
    ))
}

impl SteamApi {
    pub fn start(replies: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let replies = Arc::new(Mutex::new(VecDeque::from(replies)));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (replies, requests, stop) = (replies.clone(), requests.clone(), stop.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut chunk = [0_u8; 1024];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => request.extend_from_slice(&chunk[..read]),
                        }
                    }
                    let text = String::from_utf8_lossy(&request);
                    requests
                        .lock()
                        .unwrap()
                        .push(text.lines().next().unwrap_or_default().to_owned());
                    let reply = replies
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or_else(|| status(500));
                    let _ = stream.write_all(reply.as_bytes());
                }
            })
        };
        Self {
            base,
            replies,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    /// Queues another answer.
    pub fn push(&self, reply: String) {
        self.replies.lock().unwrap().push_back(reply);
    }

    /// The request lines seen so far, like `GET /path?query HTTP/1.1`.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for SteamApi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
