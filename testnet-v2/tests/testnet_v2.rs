use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_deribit-cli"))
        .args(args)
        .env_clear()
        .output()
        .unwrap()
}

#[test]
fn catalog_preserves_v1_and_indexes_pinned_production() {
    let v1 = run(&["coverage"]);
    let v2 = run(&["testnet", "catalog"]);
    for profile in ["mainnet", "testnet"] {
        let common = run(&["api", "--env", profile, "catalog"]);
        assert!(common.status.success());
        assert_eq!(common.stdout, v2.stdout);
    }
    assert!(v1.status.success() && v2.status.success());
    let v1: Value = serde_json::from_slice(&v1.stdout).unwrap();
    let v2: Value = serde_json::from_slice(&v2.stdout).unwrap();
    let methods = v2["methods"].as_array().unwrap();
    assert_eq!(methods.len(), 187);
    assert_eq!(v2["channels"].as_array().unwrap().len(), 43);
    assert_eq!(v1["methods"].as_array().unwrap().len(), 38);
    for row in v1["methods"].as_array().unwrap() {
        assert!(methods.iter().any(|method| method["method"] == row["api_method"] && method["v1_cli"].is_string()));
    }
    assert_eq!(
        methods
            .iter()
            .filter(|method| method["v1_cli"].is_string())
            .count(),
        38
    );
}

#[test]
fn profile_selection_and_mainnet_action_gates_fail_before_network() {
    let cases: &[(&[&str], &str)] = &[
        (&["api", "rpc", "public/get_time"], "--env"),
        (&["api", "--env", "staging", "catalog"], "invalid value"),
        (
            &[
                "api",
                "--env",
                "mainnet",
                "--endpoint",
                "wss://test.deribit.com/ws/api/v2",
                "catalog",
            ],
            "unexpected argument",
        ),
        (
            &["testnet", "--env", "mainnet", "catalog"],
            "unexpected argument",
        ),
        (
            &[
                "api",
                "--env",
                "mainnet",
                "--authenticate",
                "rpc",
                "private/get_positions",
            ],
            "explicit --scope",
        ),
        (
            &[
                "api",
                "--env",
                "mainnet",
                "--authenticate",
                "--scope",
                "connection trade:read_write",
                "--allow-write",
                "--allow-special",
                "rpc",
                "private/move_positions",
            ],
            "--allow-mainnet-write",
        ),
        (
            &[
                "api",
                "--env",
                "mainnet",
                "--authenticate",
                "--scope",
                "connection trade:read",
                "--allow-write",
                "--allow-special",
                "--allow-mainnet-write",
                "rpc",
                "private/move_positions",
            ],
            "scope is insufficient",
        ),
        (
            &[
                "api",
                "--env",
                "mainnet",
                "--authenticate",
                "--scope",
                "connection account:read",
                "--client-id-variable",
                "DERIBIT_TEST_CLIENT_ID",
                "rpc",
                "private/get_account_summary",
            ],
            "dedicated Mainnet",
        ),
        (
            &[
                "api",
                "--env",
                "testnet",
                "--authenticate",
                "--client-id-variable",
                "DERIBIT_MAINNET_CLIENT_ID",
                "rpc",
                "private/get_account_summary",
            ],
            "matching pair of Testnet",
        ),
    ];
    for (args, expected) in cases {
        let result = run(args);
        assert_eq!(result.status.code(), Some(2), "{args:?}");
        assert!(result.stdout.is_empty());
        let diagnostic = String::from_utf8(result.stderr).unwrap();
        assert!(diagnostic.contains(expected), "{args:?}: {diagnostic}");
    }
}

#[test]
fn a_testnet_only_credential_injection_does_not_enter_mainnet_auth() {
    let output = Command::new(env!("CARGO_BIN_EXE_deribit-cli"))
        .args([
            "api",
            "--env",
            "mainnet",
            "--authenticate",
            "--scope",
            "connection account:read",
            "rpc",
            "private/get_account_summary",
        ])
        .env_clear()
        .env("DERIBIT_TESTNET_CLIENT_ID", "fixture-test-id")
        .env("DERIBIT_TESTNET_CLIENT_SECRET", "fixture-test-secret")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("selected profile's credentials"));
    assert!(!diagnostic.contains("fixture-test"));
}

#[test]
fn restricts_private_writes_and_subscription_auth_before_network() {
    for args in [
        vec!["testnet", "rpc", "private/get_account_summary"],
        vec!["testnet", "--authenticate", "rpc", "private/buy"],
        vec![
            "testnet",
            "--authenticate",
            "--allow-write",
            "rpc",
            "private/withdraw",
        ],
        vec!["testnet", "subscribe", "user.portfolio.BTC"],
        vec!["testnet", "subscribe", "ticker.BTC-PERPETUAL.raw"],
        vec!["testnet", "rpc", "public/unknown"],
        vec![
            "testnet",
            "--endpoint",
            "wss://www.deribit.com/ws/api/v2",
            "rpc",
            "public/get_time",
        ],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "args={args:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn rejects_sensitive_or_ambiguous_inputs_without_echo() {
    for body in [
        concat!("{\"client_secret\":", r#""offline-sentinel""#, "}"),
        r#"{"scope":"offline-sentinel","scope":"connection"}"#,
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deribit-cli"))
            .args(["testnet", "rpc", "public/get_time", "--params-stdin"])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("offline-sentinel"));
    }
}

#[test]
fn read_checks_cannot_continue_past_a_write_error() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_deribit-cli"))
        .args([
            "testnet",
            "--authenticate",
            "--allow-write",
            "session",
            "--continue-read-errors",
        ])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"[{"method":"private/buy"}]"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("only independent read calls"));
}
