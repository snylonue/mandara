//! A tiny std-only HTTP mock server for the fetch policy tests.
//!
//! No external test dependencies: one `TcpListener`, one thread per
//! connection, plain HTTP/1.1 without keep-alive. Every request is
//! recorded (method, path, headers, body) so tests can assert what the
//! host actually sent.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// One recorded request.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// Path + query (`/hello?x=1`).
    pub url: String,
    /// Lower-cased header names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A mock HTTP server on 127.0.0.1 with an ephemeral port.
pub struct MockServer {
    pub addr: SocketAddr,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl MockServer {
    /// Start a server; `handler(path_and_query, method, headers, body)`
    /// returns `(status, response_headers, response_body)`.
    pub fn start(
        handler: impl Fn(&str, &str, &[(String, String)], &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>)
            + Send
            + Sync
            + 'static,
    ) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("mock address");
        let requests: Arc<Mutex<Vec<Request>>> = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(handler);
        let requests_for_loop = requests.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let handler = handler.clone();
                let requests = requests_for_loop.clone();
                thread::spawn(move || {
                    let _ = serve(stream, &*handler, &requests);
                });
            }
        });
        MockServer { addr, requests }
    }

    /// A URL pointing at this server (`http://127.0.0.1:port/path`).
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// `host:port` netloc of this server (for allow-list entries).
    pub fn netloc(&self) -> String {
        self.addr.to_string()
    }

    /// Everything the server has received so far.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("requests lock").clone()
    }
}

fn serve(
    mut stream: TcpStream,
    handler: &(dyn Fn(&str, &str, &[(String, String)], &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>)
          + Send
          + Sync),
    requests: &Mutex<Vec<Request>>,
) -> std::io::Result<()> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(15)))
        .ok();
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).ok();
    }

    requests.lock().expect("requests lock").push(Request {
        method: method.clone(),
        url: target.clone(),
        headers: headers.clone(),
        body: body.clone(),
    });

    let (status, response_headers, response_body) = handler(&target, &method, &headers, &body);
    let mut out = format!("HTTP/1.1 {status} {}\r\n", status_text(status));
    for (name, value) in &response_headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    if !response_headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("content-length"))
    {
        out.push_str(&format!("Content-Length: {}\r\n", response_body.len()));
    }
    out.push_str("Connection: close\r\n\r\n");
    stream.write_all(out.as_bytes())?;
    stream.write_all(&response_body)?;
    stream.flush()?;
    Ok(())
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        301 => "Moved Permanently",
        302 => "Found",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Status",
    }
}
