use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use deribit_cli::ManifestRegistry;
use serde_json::Value;

const CREDENTIALS: [(&str, &str); 3] = [
    ("DERIBIT_CLIENT_ID", "public-test-sentinel-client"),
    ("DERIBIT_CLIENT_SECRET", "public-test-sentinel-secret"),
    ("DERIBIT_API_KEY", "public-test-sentinel-key"),
];

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_deribit-cli"))
}

fn local_command(args: &[&str], credentials: bool) -> std::process::Output {
    let mut command = Command::new(binary());
    command.args(args).env_clear();
    if credentials {
        for (name, value) in CREDENTIALS {
            command.env(name, value);
        }
    }
    command.output().unwrap()
}

#[test]
fn credentials_and_read_only_environment_does_not_change_local_behavior() {
    for args in [&["coverage"][..], &["version"][..], &["--version"][..]] {
        let baseline = local_command(args, false);
        let injected = local_command(args, true);
        assert_eq!(injected.status.code(), baseline.status.code());
        assert_eq!(injected.stdout, baseline.stdout);
        assert_eq!(injected.stderr, baseline.stderr);
        for (_, sentinel) in CREDENTIALS {
            assert!(
                !injected
                    .stdout
                    .windows(sentinel.len())
                    .any(|part| part == sentinel.as_bytes())
            );
            assert!(
                !injected
                    .stderr
                    .windows(sentinel.len())
                    .any(|part| part == sentinel.as_bytes())
            );
        }
    }
}

#[test]
fn credentials_and_read_only_registry_and_coverage_are_exactly_public_read() {
    let registry = ManifestRegistry::embedded().unwrap();
    assert_eq!(registry.method_count(), 38);
    for operation in registry.operations() {
        assert!(operation.method().starts_with("public/"));
        assert!(operation.is_public_read_only());
        assert_eq!(operation.interaction(), "finite_request_response");
        let lower = operation.method().to_ascii_lowercase();
        for forbidden in ["private/", "buy", "sell", "cancel", "withdraw", "subscribe"] {
            assert!(!lower.contains(forbidden), "forbidden method {lower}");
        }
    }

    let output = local_command(&["coverage"], true);
    assert!(output.status.success());
    let coverage: Value = serde_json::from_slice(&output.stdout).unwrap();
    let methods = coverage["methods"].as_array().unwrap();
    assert_eq!(methods.len(), 38);
    assert!(methods.iter().all(|method| {
        method["auth"] == false
            && method["api_method"]
                .as_str()
                .is_some_and(|name| name.starts_with("public/"))
            && method["interaction"] == "finite_request_response"
            && method["transport"] == "http-json-rpc"
    }));
}

#[test]
fn credentials_and_read_only_proxy_observes_no_secret_or_auth_header() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut captured = Vec::new();
                    let mut chunk = [0_u8; 1024];
                    while captured.len() < 16 * 1024 {
                        match stream.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(count) => {
                                captured.extend_from_slice(&chunk[..count]);
                                if captured.windows(4).any(|window| window == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                break;
                            }
                            Err(error) => panic!("proxy read failed: {error}"),
                        }
                    }
                    stream
                        .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                        .unwrap();
                    sender.send(captured).unwrap();
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "product never reached local proxy"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("proxy accept failed: {error}"),
            }
        }
    });

    let mut command = Command::new(binary());
    command
        .args(["--timeout", "1s", "public", "get-time"])
        .env_clear()
        .env("HTTPS_PROXY", proxy);
    for (name, value) in CREDENTIALS {
        command.env(name, value);
    }
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let captured = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    server.join().unwrap();

    let request = String::from_utf8_lossy(&captured).to_ascii_lowercase();
    assert!(request.starts_with("connect www.deribit.com:443 http/1.1\r\n"));
    assert!(!request.contains("authorization:"));
    assert!(!request.contains("proxy-authorization:"));
    for (_, sentinel) in CREDENTIALS {
        assert!(!request.contains(&sentinel.to_ascii_lowercase()));
    }
}

#[test]
fn credentials_and_read_only_private_write_and_websocket_entries_are_rejected() {
    for args in [
        &["private", "get-position"][..],
        &["public", "buy"][..],
        &["public", "sell"][..],
        &["public", "subscribe"][..],
        &["websocket", "subscribe"][..],
        &["--method", "private/buy", "public", "get-time"][..],
    ] {
        let output = local_command(args, true);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}
