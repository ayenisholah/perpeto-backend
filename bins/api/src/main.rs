use funding_arb_domain::TradingMode;
use std::{
    env,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::ExitCode,
    str::FromStr,
};

const BLOCKED_ENTRY: &str = r#"{"status":"BLOCKED","reasons":["M0_SCAFFOLD_NO_TRADING_CAPABILITY"]}"#;

fn main() -> ExitCode {
    if env::args().any(|argument| argument == "--health-check") {
        return health_check();
    }

    let mode = env::var("SIGNEX_MODE").unwrap_or_else(|_| String::from("PAPER"));
    let Ok(mode) = TradingMode::from_str(&mode) else {
        eprintln!("invalid SIGNEX_MODE");
        return ExitCode::FAILURE;
    };
    if !mode.is_paper() {
        eprintln!("live mode is unavailable in the M0 scaffold");
        return ExitCode::FAILURE;
    }

    let address = env::var("SIGNEX_API_BIND").unwrap_or_else(|_| String::from("0.0.0.0:8080"));
    let listener = match TcpListener::bind(&address) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("failed to bind {address}: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("Signex scaffold API listening on {address} in PAPER mode");
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => respond(&mut stream),
            Err(error) => eprintln!("connection failed: {error}"),
        }
    }
    ExitCode::SUCCESS
}

fn health_check() -> ExitCode {
    match TcpStream::connect("127.0.0.1:8080") {
        Ok(mut stream) => {
            let request = b"GET /livez HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
            if stream.write_all(request).is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(_) => ExitCode::FAILURE,
    }
}

fn respond(stream: &mut TcpStream) {
    let mut request = [0_u8; 2048];
    let Ok(length) = stream.read(&mut request) else {
        return;
    };
    let first_line = String::from_utf8_lossy(&request[..length]);
    let path = first_line.split_whitespace().nth(1).unwrap_or("/");
    let (status, body) = match path {
        "/livez" | "/readyz" => ("200 OK", r#"{"status":"OK","mode":"PAPER","stage":"M0_SCAFFOLD"}"#),
        "/entryz" => ("503 Service Unavailable", BLOCKED_ENTRY),
        _ => ("404 Not Found", r#"{"error":"NOT_FOUND"}"#),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}
