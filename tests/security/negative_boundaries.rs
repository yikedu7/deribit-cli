use std::{ffi::OsStr, path::PathBuf, process::Command};

use deribit_cli::{
    ManifestRegistry, ParameterInput, ProxyEnvironment, ProxySettings, build_cli,
    resolve_parameters,
};

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_deribit-cli"))
}

fn rejected<I, S>(args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(binary())
        .args(args)
        .env_clear()
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

fn all_argument_ids(command: &clap::Command) -> Vec<String> {
    let mut ids = command
        .get_arguments()
        .map(|argument| argument.get_id().as_str().to_owned())
        .collect::<Vec<_>>();
    for child in command.get_subcommands() {
        ids.extend(all_argument_ids(child));
    }
    ids
}

#[test]
fn private_generic_manifest_endpoint_tls_and_expansion_injection_are_rejected() {
    for args in [
        vec!["private", "get-position"],
        vec!["public", "call", "--method", "public/get_time"],
        vec!["--method", "public/get_time", "public", "get-time"],
        vec!["--manifest", "forged.json", "public", "get-time"],
        vec![
            "--endpoint",
            "https://example.invalid",
            "public",
            "get-time",
        ],
        vec!["--url", "https://example.invalid", "public", "get-time"],
        vec!["--insecure", "public", "get-time"],
        vec!["--retry", "2", "public", "get-time"],
        vec!["--template", "${HOME}", "public", "get-time"],
        vec!["--include", "params.json", "public", "get-time"],
        vec!["websocket", "subscribe"],
    ] {
        rejected(args);
    }

    let registry = ManifestRegistry::embedded().unwrap();
    let ids = all_argument_ids(&build_cli(&registry).unwrap());
    for forbidden in [
        "method",
        "manifest",
        "endpoint",
        "url",
        "insecure",
        "retry",
        "template",
        "include",
        "client-id",
        "client-secret",
        "api-key",
        "token",
    ] {
        assert!(
            !ids.iter().any(|id| id == forbidden),
            "forbidden argument {forbidden}"
        );
    }
}

#[test]
fn proxy_userinfo_file_and_arbitrary_schemes_are_rejected() {
    for raw in [
        "https://user:secret@proxy.invalid:8443",
        "http://user@proxy.invalid:8080",
        "file:///tmp/proxy.sock",
        "ftp://proxy.invalid",
        "socks5://proxy.invalid:1080",
        "javascript:alert(1)",
    ] {
        let error = ProxySettings::from_environment(ProxyEnvironment {
            https_proxy: Some(raw.to_owned()),
            ..ProxyEnvironment::default()
        })
        .unwrap_err();
        let diagnostic = error.to_string();
        assert!(!diagnostic.contains("secret"));
    }
}

#[test]
fn duplicate_keys_depth_and_size_limits_are_rejected_before_transport() {
    let registry = ManifestRegistry::embedded().unwrap();
    let operation = registry.operation("public_get_order_book").unwrap();

    let duplicate = ParameterInput::Inline(
        r#"{"instrument_name":"BTC-PERPETUAL","instrument_name":"ETH-PERPETUAL"}"#.to_owned(),
    );
    let error = resolve_parameters(operation, duplicate, &mut &b""[..], false).unwrap_err();
    assert!(error.to_string().contains("duplicate JSON object key"));

    let too_deep = format!(
        "{{\"instrument_name\":\"BTC-PERPETUAL\",\"future\":{}0{}}}",
        "[".repeat(64),
        "]".repeat(64)
    );
    let error = resolve_parameters(
        operation,
        ParameterInput::Inline(too_deep),
        &mut &b""[..],
        false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("nesting exceeds"));

    let oversized = format!(
        "{{\"instrument_name\":\"{}\"}}",
        "x".repeat(deribit_cli::MAX_PARAMETER_BYTES)
    );
    let error = resolve_parameters(
        operation,
        ParameterInput::Inline(oversized),
        &mut &b""[..],
        false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("raw parameters exceed"));
}

#[test]
fn redirect_retry_and_implicit_proxy_discovery_remain_disabled() {
    let source = include_str!("../../src/reqwest_transport.rs");
    assert!(source.contains(".redirect(redirect::Policy::none())"));
    assert!(source.contains(".retry(retry::never())"));
    assert!(source.contains(".no_proxy()"));

    let registry = ManifestRegistry::embedded().unwrap();
    assert_eq!(registry.method_count(), 38);
    assert!(
        registry
            .operations()
            .all(|operation| operation.is_public_read_only())
    );
}
