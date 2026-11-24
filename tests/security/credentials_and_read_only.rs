use std::{path::PathBuf, process::Command};

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
    for args in [
        &["catalog"][..],
        &["completion", "bash"][..],
        &["--version"][..],
    ] {
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
fn credentials_and_read_only_registry_and_catalog_keep_public_reads_typed() {
    let registry = ManifestRegistry::embedded().unwrap();
    assert_eq!(registry.method_count(), 38);
    for operation in registry.operations() {
        assert!(operation.method().starts_with("public/"));
        assert!(operation.is_public_read_only());
        assert_eq!(operation.interaction(), "finite_request_response");
    }

    let output = local_command(&["catalog"], true);
    assert!(output.status.success());
    let catalog: Value = serde_json::from_slice(&output.stdout).unwrap();
    let methods = catalog["methods"].as_array().unwrap();
    assert_eq!(methods.len(), 187);
    assert!(
        methods
            .iter()
            .any(|method| { method["method"] == "private/buy" && method["effect"] == "write" })
    );
    assert_eq!(
        methods
            .iter()
            .filter(|method| method["cli_command"]
                .as_array()
                .is_some_and(|command| command.len() == 2))
            .count(),
        39
    );
}

#[test]
fn credentials_and_read_only_proxy_does_not_change_websocket_target() {
    let output = local_command(
        &[
            "--timeout-seconds",
            "1",
            "--env",
            "mainnet",
            "rpc",
            "call",
            "public/get_time",
        ],
        true,
    );
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(
        matches!(output.status.code(), Some(5 | 124)),
        "{diagnostic}"
    );
    assert!(output.stdout.is_empty());
    assert!(!diagnostic.contains("public-test-sentinel"));
    assert!(!diagnostic.contains("HTTPS_PROXY"));
}

#[test]
fn credentials_and_read_only_removed_and_unscoped_entries_are_rejected() {
    for args in [
        &["private", "get-position"][..],
        &["market", "buy"][..],
        &["market", "sell"][..],
        &["market", "subscribe"][..],
        &["websocket", "subscribe"][..],
        &["--method", "private/buy", "market", "time"][..],
    ] {
        let output = local_command(args, true);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}
