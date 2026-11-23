#![cfg(feature = "general-api")]

use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deribit-cli"));
    command.env_clear();
    command
}

fn run(args: &[&str]) -> Output {
    cli().args(args).output().unwrap()
}

fn run_with_stdin(args: &[&str], input: &str) -> Output {
    let mut child = cli()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn assert_rejected(output: Output, expected: &str, args: &[&str]) -> String {
    assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
    assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains(expected), "{args:?}: {diagnostic}");
    diagnostic
}

fn read_json(args: &[&str]) -> Value {
    let output = run(args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn catalog_is_local_and_preserves_all_v1_mappings() {
    let v1 = read_json(&["coverage"]);
    let catalog = read_json(&["catalog"]);
    let methods = catalog["methods"].as_array().unwrap();
    let channels = catalog["channels"].as_array().unwrap();
    assert_eq!(methods.len(), 187);
    assert_eq!(channels.len(), 43);
    assert_eq!(
        methods
            .iter()
            .filter(|row| row["testnet_verified"] == true)
            .count(),
        145
    );
    assert_eq!(
        methods
            .iter()
            .filter(|row| row["group"] == "Trading" && row["testnet_verified"] == true)
            .count(),
        37
    );
    assert_eq!(
        channels
            .iter()
            .filter(|row| row["testnet_acknowledged"] == true)
            .count(),
        41
    );
    assert_eq!(
        channels
            .iter()
            .filter(|row| row["testnet_event_received"] == true)
            .count(),
        27
    );
    assert!(channels.iter().all(|row| {
        row["testnet_event_received"] != true || row["testnet_acknowledged"] == true
    }));
    assert_eq!(v1["methods"].as_array().unwrap().len(), 38);
    for row in v1["methods"].as_array().unwrap() {
        assert!(methods.iter().any(|method| {
            method["method"] == row["api_method"] && method["v1_cli"].is_string()
        }));
    }
    assert_eq!(
        methods
            .iter()
            .filter(|method| method["v1_cli"].is_string())
            .count(),
        38
    );

    let ticker = read_json(&["catalog", "method", "public/ticker"]);
    assert_eq!(
        ticker["cli_command"],
        serde_json::json!(["market", "ticker"])
    );
    let method = read_json(&["catalog", "method", "private/move_positions"]);
    assert_eq!(method["method"], "private/move_positions");
    assert_eq!(method["effect"], "special");
    assert_eq!(method["testnet_verified"], true);
    let template = "ticker.(instrument_name).(interval)";
    let channel = read_json(&["catalog", "channel", template]);
    assert_eq!(channel["channel"], template);
    assert_eq!(channel["testnet_acknowledged"], true);
    for args in [
        ["catalog", "method", "private/not_a_method"],
        ["catalog", "channel", "not.a.channel"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
    }
}

#[test]
fn root_and_command_help_make_domain_and_environment_available_locally() {
    for flag in ["-h", "--help"] {
        let top = run(&[flag]);
        assert!(top.status.success(), "{top:?}");
        let help = String::from_utf8(top.stdout).unwrap();
        for name in [
            "public",
            "market",
            "orders",
            "portfolio",
            "account",
            "wallet",
            "stream",
            "rpc",
            "catalog",
            "--env",
        ] {
            assert!(help.contains(name), "{name}: {help}");
        }
    }
    for (args, text) in [
        (&["market", "ticker", "--help"][..], "--instrument-name"),
        (&["market", "order-book", "--help"][..], "--depth"),
        (&["portfolio", "positions", "--help"][..], "--currency"),
        (&["account", "summary", "--help"][..], "--currency"),
        (&["wallet", "transfers", "--help"][..], "--currency"),
        (&["rpc", "call", "--help"][..], "--params-file"),
        (&["rpc", "session", "--help"][..], "--file"),
        (&["stream", "subscribe", "--help"][..], "--observe-seconds"),
    ] {
        let output = run(args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains(text), "{args:?}: {help}");
    }
}

#[test]
fn shell_completion_includes_domain_commands_and_preserves_public() {
    let args = ["completion", "bash"];
    let output = run(&args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
    let script = String::from_utf8(output.stdout).unwrap();
    for word in ["public", "market", "order-book", "--authenticate"] {
        assert!(script.contains(word), "{word}: {script}");
    }
}

#[test]
fn all_new_network_groups_require_an_explicit_environment() {
    for args in [
        &["market", "ticker", "--instrument-name", "BTC-PERPETUAL"][..],
        &["orders", "list"][..],
        &["portfolio", "positions", "--currency", "BTC"][..],
        &["account", "summary", "--currency", "BTC"][..],
        &["wallet", "transfers", "--currency", "BTC"][..],
        &["stream", "subscribe", "ticker.BTC-PERPETUAL.100ms"][..],
        &["rpc", "call", "public/get_time"][..],
        &["rpc", "session"][..],
    ] {
        assert_rejected(run(args), "--env", args);
    }
    let args = [
        "--env",
        "testnet",
        "market",
        "order-book",
        "--instrument-name",
        "BTC-PERPETUAL",
        "--depth",
        "not-an-integer",
    ];
    assert_rejected(run(&args), "--depth", &args);
}

#[test]
fn profile_selection_and_mainnet_action_gates_fail_before_network() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["--env", "staging", "rpc", "call", "public/get_time"],
            "invalid value",
        ),
        (
            &[
                "--env",
                "testnet",
                "--env",
                "mainnet",
                "rpc",
                "call",
                "public/get_time",
            ],
            "cannot be used multiple times",
        ),
        (
            &[
                "--env",
                "mainnet",
                "--endpoint",
                "wss://test.deribit.com/ws/api/v2",
                "rpc",
                "call",
                "public/get_time",
            ],
            "unexpected argument",
        ),
        (
            &[
                "--env",
                "testnet",
                "--allow-mainnet-write",
                "rpc",
                "call",
                "public/get_time",
            ],
            "--allow-mainnet-write requires --env mainnet",
        ),
        (
            &["--env", "mainnet", "rpc", "call", "public/unknown"],
            "absent from the pinned Deribit API catalog",
        ),
        (
            &[
                "--env",
                "mainnet",
                "--authenticate",
                "portfolio",
                "positions",
            ],
            "explicit --scope",
        ),
        (
            &[
                "--env",
                "mainnet",
                "--authenticate",
                "--scope",
                "connection account:read",
                "portfolio",
                "positions",
            ],
            "scope is insufficient",
        ),
        (
            &[
                "--env",
                "mainnet",
                "--authenticate",
                "--scope",
                "connection account:read",
                "rpc",
                "call",
                "private/get_positions",
            ],
            "scope is insufficient",
        ),
        (
            &[
                "--env",
                "mainnet",
                "--authenticate",
                "--scope",
                "connection trade:read_write",
                "--allow-write",
                "--allow-special",
                "rpc",
                "call",
                "private/move_positions",
            ],
            "--allow-mainnet-write",
        ),
        (
            &[
                "--env",
                "mainnet",
                "--authenticate",
                "--scope",
                "connection trade:read",
                "--allow-write",
                "--allow-special",
                "--allow-mainnet-write",
                "rpc",
                "call",
                "private/move_positions",
            ],
            "scope is insufficient",
        ),
        (
            &[
                "--env",
                "mainnet",
                "--authenticate",
                "--scope",
                "connection account:read",
                "--client-id-variable",
                "DERIBIT_TEST_CLIENT_ID",
                "account",
                "summary",
                "--currency",
                "BTC",
            ],
            "dedicated Mainnet",
        ),
        (
            &[
                "--env",
                "testnet",
                "--authenticate",
                "--client-id-variable",
                "DERIBIT_MAINNET_CLIENT_ID",
                "account",
                "summary",
                "--currency",
                "BTC",
            ],
            "matching pair of Testnet",
        ),
    ];
    for (args, expected) in cases {
        assert_rejected(run(args), expected, args);
    }
}

#[test]
fn credentials_from_the_other_profile_do_not_enter_auth() {
    for (profile, wrong_profile_prefix) in [
        ("mainnet", "DERIBIT_TESTNET"),
        ("testnet", "DERIBIT_MAINNET"),
    ] {
        let args = [
            "--env",
            profile,
            "--authenticate",
            "--scope",
            "connection account:read",
            "account",
            "summary",
            "--currency",
            "BTC",
        ];
        let output = cli()
            .args(args)
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
        let diagnostic = assert_rejected(output, "selected profile's credentials", &args);
        assert!(!diagnostic.contains("fixture-test"));
    }
}

#[test]
fn restricts_private_writes_and_subscription_auth_before_network() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["--env", "testnet", "rpc", "call", "private/get_open_orders"],
            "--authenticate",
        ),
        (&["--env", "testnet", "orders", "list"], "--authenticate"),
        (
            &[
                "--env",
                "testnet",
                "wallet",
                "transfers",
                "--currency",
                "BTC",
            ],
            "--authenticate",
        ),
        (
            &[
                "--env",
                "testnet",
                "--authenticate",
                "rpc",
                "call",
                "private/buy",
            ],
            "--allow-write",
        ),
        (
            &[
                "--env",
                "testnet",
                "--authenticate",
                "--allow-write",
                "rpc",
                "call",
                "private/withdraw",
            ],
            "--allow-special",
        ),
        (
            &[
                "--env",
                "testnet",
                "stream",
                "subscribe",
                "user.portfolio.BTC",
            ],
            "--authenticate",
        ),
        (
            &[
                "--env",
                "testnet",
                "stream",
                "subscribe",
                "ticker.BTC-PERPETUAL.raw",
            ],
            "--authenticate",
        ),
    ];
    for (args, expected) in cases {
        assert_rejected(run(args), expected, args);
    }
}

#[test]
fn rpc_rejects_sensitive_or_ambiguous_input_without_echo() {
    let args = [
        "--env",
        "testnet",
        "rpc",
        "call",
        "public/get_time",
        "--params-stdin",
    ];
    let sensitive = concat!("{\"client_secret\":", r#""offline-sentinel""#, "}");
    let duplicated = r#"{"scope":"offline-sentinel","scope":"connection"}"#;
    for (body, expected) in [(sensitive, "authentication fields"), (duplicated, "JSON")] {
        let diagnostic = assert_rejected(run_with_stdin(&args, body), expected, &args);
        assert!(!diagnostic.contains("offline-sentinel"));
    }

    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), sensitive).unwrap();
    let file_args = [
        "--env",
        "testnet",
        "rpc",
        "call",
        "public/get_time",
        "--params-file",
        file.path().to_str().unwrap(),
    ];
    let diagnostic = assert_rejected(run(&file_args), "authentication fields", &file_args);
    assert!(!diagnostic.contains("offline-sentinel"));

    let mut conflicting = file_args.to_vec();
    conflicting.push("--params-stdin");
    assert_rejected(run(&conflicting), "cannot be used with", &conflicting);

    let wrong_type_args = [
        "--env",
        "testnet",
        "rpc",
        "call",
        "public/get_order_book",
        "--params-stdin",
    ];
    let params = r#"{"instrument_name":"BTC-PERPETUAL","depth":"5"}"#;
    assert_rejected(
        run_with_stdin(&wrong_type_args, params),
        "wrong JSON type",
        &wrong_type_args,
    );
}

#[test]
fn read_checks_cannot_continue_past_a_write_error() {
    let args = [
        "--env",
        "testnet",
        "--authenticate",
        "--allow-write",
        "rpc",
        "session",
        "--continue-read-errors",
    ];
    let steps = r#"[{"method":"private/buy"}]"#;
    let diagnostic = "only independent read calls";
    assert_rejected(run_with_stdin(&args, steps), diagnostic, &args);
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), steps).unwrap();
    let mut file_args = args.to_vec();
    file_args.extend(["--file", file.path().to_str().unwrap()]);
    assert_rejected(run(&file_args), diagnostic, &file_args);
}

#[test]
fn public_keeps_legacy_dispatch_when_a_flag_value_matches_a_new_group() {
    let empty_dir = tempfile::tempdir().unwrap();
    let args = [
        "--env",
        "testnet",
        "--params-file",
        "rpc",
        "public",
        "get-time",
    ];
    let output = cli()
        .current_dir(empty_dir.path())
        .args(args)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("rpc"));
}
