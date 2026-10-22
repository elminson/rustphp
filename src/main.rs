//! `rustphp` — a clean-room reimplementation of the PHP/FI 1.99s interpreter.
//!
//! Runs a PHP/FI template either from the command line (`rustphp script.html`)
//! or as a CGI program (when `GATEWAY_INTERFACE` / `REQUEST_METHOD` are set),
//! in which case GET/POST form fields and the CGI environment are exposed as
//! global `$variables`, and a default `Content-type` header is emitted.

mod ast;
mod builtins;
mod interp;
mod lexer;
mod parser;
mod server;
mod token;
mod value;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use interp::Interp;
use value::Value;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if matches!(args.get(1).map(String::as_str), Some("--serve") | Some("-s") | Some("serve")) {
        return run_server(&args[2..]);
    }

    let is_cgi = std::env::var_os("GATEWAY_INTERFACE").is_some()
        || std::env::var_os("REQUEST_METHOD").is_some();

    let script_path = match locate_script(is_cgi) {
        Some(p) => p,
        None => {
            eprintln!("usage: rustphp <script.html>");
            return ExitCode::from(2);
        }
    };

    let source = match std::fs::read(&script_path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => {
            eprintln!("rustphp: cannot read {}: {e}", script_path.display());
            return ExitCode::from(2);
        }
    };
    // A CLI shebang line (`#!...`) is not part of the template.
    let source = strip_shebang(&source);

    let tokens = match lexer::lex(&source) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("rustphp: lex error in {}: {e}", script_path.display());
            return ExitCode::FAILURE;
        }
    };
    if std::env::var_os("RUSTPHP_DUMP_TOKENS").is_some() {
        for (i, t) in tokens.iter().enumerate() {
            eprintln!("{i}: {t:?}");
        }
    }
    let program = match parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("rustphp: parse error in {}: {e}", script_path.display());
            return ExitCode::FAILURE;
        }
    };

    let include_dir = script_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut interp = Interp::new(include_dir);

    if is_cgi {
        seed_cgi_variables(&mut interp);
    } else {
        seed_cli_variables(&mut interp, &script_path);
    }

    interp.run(&program);

    if is_cgi {
        let has_content_type = interp
            .headers
            .iter()
            .any(|h| h.to_ascii_lowercase().starts_with("content-type:"));
        if !has_content_type {
            print!("Content-type: text/html; charset=utf-8\r\n");
        }
        for h in &interp.headers {
            print!("{h}\r\n");
        }
        print!("\r\n");
    }
    print!("{}", interp.out);
    ExitCode::SUCCESS
}

/// `rustphp --serve [ADDR|PORT] [DOCROOT]`: start the development server.
fn run_server(rest: &[String]) -> ExitCode {
    let mut addr = "127.0.0.1:8000".to_string();
    let mut docroot: Option<PathBuf> = None;
    for a in rest {
        if !a.is_empty() && a.chars().all(|c| c.is_ascii_digit()) {
            addr = format!("127.0.0.1:{a}");
        } else if a.contains(':') && !Path::new(a).exists() {
            addr = a.clone();
        } else {
            docroot = Some(PathBuf::from(a));
        }
    }
    let docroot = docroot
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    match server::serve(&addr, docroot) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rustphp: server error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Find the script to run: an explicit CLI argument, or the CGI server's
/// `SCRIPT_FILENAME` / `PATH_TRANSLATED`.
fn locate_script(is_cgi: bool) -> Option<PathBuf> {
    if let Some(arg) = std::env::args().nth(1) {
        return Some(PathBuf::from(arg));
    }
    if is_cgi {
        for key in ["SCRIPT_FILENAME", "PATH_TRANSLATED"] {
            if let Some(v) = std::env::var_os(key) {
                return Some(PathBuf::from(v));
            }
        }
    }
    None
}

fn strip_shebang(src: &str) -> String {
    if src.starts_with("#!") {
        match src.find('\n') {
            Some(i) => src[i + 1..].to_string(),
            None => String::new(),
        }
    } else {
        src.to_string()
    }
}

/// Expose the CGI environment and decoded GET/POST fields as global variables.
fn seed_cgi_variables(interp: &mut Interp) {
    for (k, v) in std::env::vars() {
        interp.set_global(&k, Value::Str(v));
    }
    if let Some(qs) = std::env::var_os("QUERY_STRING") {
        parse_form(&qs.to_string_lossy(), interp);
    }
    if std::env::var("REQUEST_METHOD").map(|m| m.eq_ignore_ascii_case("POST")).unwrap_or(false) {
        let mut body = String::new();
        if std::io::stdin().read_to_string(&mut body).is_ok() {
            parse_form(&body, interp);
        }
    }
}

fn seed_cli_variables(interp: &mut Interp, script: &Path) {
    // Make the usual pieces available even under the CLI.
    interp.set_global("SCRIPT_NAME", Value::Str(script.display().to_string()));
    interp.set_global("SCRIPT_FILENAME", Value::Str(script.display().to_string()));
    for (k, v) in std::env::vars() {
        interp.set_global(&k, Value::Str(v));
    }
}

/// Parse `a=1&b=hello;c=x` form data into global variables (URL-decoded).
fn parse_form(data: &str, interp: &mut Interp) {
    for pair in data.split(['&', ';']) {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (url_decode(k), url_decode(v)),
            None => (url_decode(pair), String::new()),
        };
        if !k.is_empty() {
            interp.set_global(&k, Value::Str(v));
        }
    }
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
