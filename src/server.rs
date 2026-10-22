//! A tiny, dependency-free HTTP/1.1 development server for rustphp.
//!
//! It is the rough equivalent of `php -S`: each request for a template file
//! (`.php`, `.phtml`, `.html`, `.htm`) is run through the interpreter with
//! CGI-style globals (`REQUEST_METHOD`, `QUERY_STRING`, `HTTP_*` headers, and
//! decoded GET/POST form fields); any other file is served as a static asset.
//! One thread per connection, `Connection: close`, no keep-alive — plenty for
//! local development, not meant to face the public internet.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use crate::interp::Interp;
use crate::value::Value;
use crate::{lexer, parser};

const TEMPLATE_EXTS: &[&str] = &["php", "phtml", "html", "htm"];
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

pub fn serve(addr: &str, docroot: PathBuf) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    let root = docroot.canonicalize().unwrap_or(docroot);
    eprintln!(
        "rustphp dev server listening on http://{addr}\n  document root: {}\n  (Ctrl-C to stop)",
        root.display()
    );
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let root = root.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle(s, &root) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
    Ok(())
}

struct Request {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: String,
    peer: String,
}

fn handle(stream: TcpStream, docroot: &Path) -> std::io::Result<()> {
    let peer = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    let req = match read_request(&mut reader, peer) {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(()), // empty / closed connection
        Err(msg) => return send(&mut writer, 400, "text/html", &format!("<h1>400 Bad Request</h1><p>{msg}</p>")),
    };

    // Log the request line.
    eprintln!("{} {}", req.method, req.target);

    let (path, query) = match req.target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (req.target.clone(), String::new()),
    };
    let decoded_path = crate::url_decode(&path);

    let file = match resolve(docroot, &decoded_path) {
        Some(f) => f,
        None => {
            return send(&mut writer, 404, "text/html", &format!("<h1>404 Not Found</h1><p>{decoded_path}</p>"));
        }
    };

    if is_template(&file) {
        serve_template(&mut writer, &file, &decoded_path, &query, &req)
    } else {
        serve_static(&mut writer, &file)
    }
}

/// Parse the request line, headers, and (for POST) the body.
fn read_request<R: BufRead>(reader: &mut R, peer: String) -> Result<Option<Request>, String> {
    let mut line = String::new();
    let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(None);
    }
    let mut parts = line.trim_end().split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    if method.is_empty() {
        return Err("empty request line".into());
    }

    let mut headers = HashMap::new();
    let mut header_bytes = 0;
    loop {
        let mut h = String::new();
        let got = reader.read_line(&mut h).map_err(|e| e.to_string())?;
        if got == 0 {
            break;
        }
        header_bytes += got;
        if header_bytes > MAX_HEADER_BYTES {
            return Err("request headers too large".into());
        }
        let trimmed = h.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let mut body = String::new();
    if let Some(len) = headers.get("content-length").and_then(|v| v.parse::<usize>().ok()) {
        let len = len.min(MAX_BODY_BYTES);
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
        body = String::from_utf8_lossy(&buf).into_owned();
    }

    Ok(Some(Request {
        method,
        target,
        headers,
        body,
        peer,
    }))
}

/// Map a request path to a file under the document root, rejecting traversal.
fn resolve(docroot: &Path, path: &str) -> Option<PathBuf> {
    let rel = path.trim_start_matches('/');
    let mut candidate = docroot.join(rel);
    if candidate.is_dir() {
        candidate = candidate.join("index.html");
    }
    let real = candidate.canonicalize().ok()?;
    // Must stay inside the document root.
    if !real.starts_with(docroot) {
        return None;
    }
    if real.is_file() {
        Some(real)
    } else {
        None
    }
}

fn is_template(file: &Path) -> bool {
    file.extension()
        .and_then(|e| e.to_str())
        .map(|e| TEMPLATE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn serve_template(
    writer: &mut TcpStream,
    file: &Path,
    script_name: &str,
    query: &str,
    req: &Request,
) -> std::io::Result<()> {
    let source = match std::fs::read(file) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => return send(writer, 500, "text/html", &format!("<h1>500</h1><p>cannot read script: {e}</p>")),
    };

    let program = match lexer::lex(&source).and_then(parser::parse) {
        Ok(p) => p,
        Err(e) => {
            return send(writer, 500, "text/html", &format!("<h1>500 Parse Error</h1><pre>{e}</pre>"));
        }
    };

    let include_dir = file.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let mut interp = Interp::new(include_dir);

    // CGI-style server variables.
    interp.set_global("REQUEST_METHOD", Value::Str(req.method.clone()));
    interp.set_global("QUERY_STRING", Value::Str(query.to_string()));
    interp.set_global("SCRIPT_NAME", Value::Str(script_name.to_string()));
    interp.set_global("PHP_SELF", Value::Str(script_name.to_string()));
    interp.set_global("SCRIPT_FILENAME", Value::Str(file.display().to_string()));
    interp.set_global("SERVER_SOFTWARE", Value::Str(concat!("rustphp/", env!("CARGO_PKG_VERSION")).into()));
    interp.set_global("GATEWAY_INTERFACE", Value::Str("CGI/1.1".into()));
    interp.set_global("REMOTE_ADDR", Value::Str(req.peer.clone()));
    // Request headers become HTTP_* variables (e.g. User-Agent -> HTTP_USER_AGENT).
    for (k, v) in &req.headers {
        let name = format!("HTTP_{}", k.to_ascii_uppercase().replace('-', "_"));
        interp.set_global(&name, Value::Str(v.clone()));
    }

    // GET and POST form fields as globals.
    crate::parse_form(query, &mut interp);
    if req.method.eq_ignore_ascii_case("POST") {
        let ct = req.headers.get("content-type").map(String::as_str).unwrap_or("");
        if ct.starts_with("application/x-www-form-urlencoded") {
            crate::parse_form(&req.body, &mut interp);
        }
    }

    interp.run(&program);

    let has_ct = interp
        .headers
        .iter()
        .any(|h| h.to_ascii_lowercase().starts_with("content-type:"));
    let mut head = String::new();
    head.push_str("HTTP/1.1 200 OK\r\n");
    if !has_ct {
        head.push_str("Content-Type: text/html\r\n");
    }
    for h in &interp.headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str(&format!("Content-Length: {}\r\n", interp.out.as_bytes().len()));
    head.push_str("Connection: close\r\n\r\n");

    writer.write_all(head.as_bytes())?;
    writer.write_all(interp.out.as_bytes())?;
    writer.flush()
}

fn serve_static(writer: &mut TcpStream, file: &Path) -> std::io::Result<()> {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => return send(writer, 500, "text/html", &format!("<h1>500</h1><p>{e}</p>")),
    };
    let ct = content_type(file);
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    writer.write_all(head.as_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "css" => "text/css",
        "js" => "text/javascript",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn send(writer: &mut TcpStream, status: u16, ct: &str, body: &str) -> std::io::Result<()> {
    let reason = match status {
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.as_bytes().len()
    );
    writer.write_all(head.as_bytes())?;
    writer.write_all(body.as_bytes())?;
    writer.flush()
}
