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
fn catalog_preserves_v1_and_is_shared_by_both_profiles_and_the_alias() {
    let v1 = run(&["coverage"]);
    let legacy_catalog = run(&["testnet", "catalog"]);
    for profile in ["mainnet", "testnet"] {
        let common = run(&["api", "--env", profile, "catalog"]);
        assert!(common.status.success());
        assert_eq!(common.stdout, legacy_catalog.stdout);
    }
    assert!(v1.status.success() && legacy_catalog.status.success());
    let v1: Value = serde_json::from_slice(&v1.stdout).unwrap();
    let catalog: Value = serde_json::from_slice(&legacy_catalog.stdout).unwrap();
    let methods = catalog["methods"].as_array().unwrap();
    assert_eq!(methods.len(), 187);
    assert_eq!(catalog["channels"].as_array().unwrap().len(), 43);
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
fn help_distinguishes_explicit_profile_selection_from_the_testnet_alias() {
    for flag in ["-h", "--help"] {
        let top = run(&[flag]);
        assert!(top.status.success());
        let help = String::from_utf8(top.stdout).unwrap();
        assert!(help.contains("deribit-cli api --env <mainnet|testnet>"));
        assert!(help.contains("deribit-cli testnet --help"));
    }
    let api = run(&["api", "--help"]);
    let testnet = run(&["testnet", "--help"]);
    assert!(api.status.success() && testnet.status.success());
    let api = String::from_utf8(api.stdout).unwrap();
    let testnet = String::from_utf8(testnet.stdout).unwrap();
    assert!(api.contains("--env") && api.contains("mainnet, testnet"));
    assert!(api.contains("--allow-mainnet-write"));
    assert!(testnet.contains("fixed to official Deribit Testnet"));
    assert!(!testnet.contains("--env") && !testnet.contains("--allow-mainnet-write"));
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
                "testnet",
                "--allow-mainnet-write",
                "catalog",
            ],
            "--allow-mainnet-write requires --env mainnet",
        ),
        (
            &["api", "--env", "mainnet", "rpc", "public/unknown"],
            "absent from the pinned Deribit API catalog",
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
fn credentials_from_the_other_profile_do_not_enter_auth() {
    for (profile, wrong_profile_prefix) in [
        ("mainnet", "DERIBIT_TESTNET"),
        ("testnet", "DERIBIT_MAINNET"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_deribit-cli"))
            .args([
                "api",
                "--env",
                profile,
                "--authenticate",
                "--scope",
                "connection account:read",
                "rpc",
                "private/get_account_summary",
            ])
            .env_clear()
            .env(
                format!("{wrong_profile_prefix}_CLIENT_ID"),
                "fixture-test-id",
            )
            .env(
                format!("{wrong_profile_prefix}_CLIENT_SECRET"),
                "fixture-test-secret",
            )
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{profile}");
        assert!(output.stdout.is_empty());
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(
            diagnostic.contains("selected profile's credentials"),
            "{profile}"
        );
        assert!(!diagnostic.contains("fixture-test"));
    }
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
