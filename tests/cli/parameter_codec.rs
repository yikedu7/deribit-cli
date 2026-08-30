use std::{collections::HashMap, fs, io::Cursor};

use deribit_cli::{
    CliCommand, ErrorClass, ManifestRegistry, ParameterError, ParameterInput,
    parse_from_with_stdin, resolve_parameters,
};
use serde_json::json;

fn operation<'a>(
    registry: &'a ManifestRegistry,
    command: &str,
) -> deribit_cli::ValidatedOperation<'a> {
    registry.canonical_command(command).unwrap()
}

fn typed(values: &[(&str, &str)]) -> ParameterInput {
    ParameterInput::Typed(
        values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect::<HashMap<_, _>>(),
    )
}

fn resolve(
    operation: deribit_cli::ValidatedOperation<'_>,
    input: ParameterInput,
) -> Result<serde_json::Value, ParameterError> {
    resolve_parameters(operation, input, &mut Cursor::new(Vec::new()), false)
}

#[test]
fn parameter_codec_direct_flags_preserve_types_enums_and_omission() {
    let registry = ManifestRegistry::embedded().unwrap();
    let instruments = operation(&registry, "public get-instruments");
    let params = resolve(
        instruments,
        typed(&[("currency", "BTC"), ("expired", "false")]),
    )
    .unwrap();
    assert_eq!(params, json!({"currency":"BTC","expired":false}));
    assert!(
        params.get("kind").is_none(),
        "upstream defaults stay omitted"
    );

    let order_book = operation(&registry, "public get-order-book");
    let params = resolve(
        order_book,
        typed(&[("instrument_name", "BTC-PERPETUAL"), ("depth", "5")]),
    )
    .unwrap();
    assert_eq!(params, json!({"instrument_name":"BTC-PERPETUAL","depth":5}));

    for bad in ["05", "-0", "1.0", "9223372036854775808"] {
        let error = resolve(
            order_book,
            typed(&[("instrument_name", "BTC-PERPETUAL"), ("depth", bad)]),
        )
        .unwrap_err();
        assert_eq!(error.class(), ErrorClass::Usage);
    }
    let error = resolve(instruments, typed(&[("currency", "btc")])).unwrap_err();
    assert_eq!(error.class(), ErrorClass::Usage);
    let error = resolve(instruments, typed(&[("expired", "1")])).unwrap_err();
    assert_eq!(error.class(), ErrorClass::Usage);
    let error = resolve(instruments, typed(&[("currency", "BTC"), ("unknown", "x")])).unwrap_err();
    assert_eq!(error.class(), ErrorClass::Usage);

    let announcements = operation(&registry, "public get-announcements");
    let too_many = resolve(announcements, typed(&[("count", "51")])).unwrap_err();
    assert_eq!(too_many.class(), ErrorClass::Usage);

    let block_rfq = operation(&registry, "public get-block-rfq-trades");
    for rejected in ["5", "9", "51", "1000"] {
        let error = resolve(
            block_rfq,
            typed(&[("currency", "BTC"), ("count", rejected)]),
        )
        .unwrap_err();
        assert_eq!(error.class(), ErrorClass::Usage, "count={rejected}");
    }
    for accepted in ["10", "50"] {
        let params = resolve(
            block_rfq,
            typed(&[("currency", "BTC"), ("count", accepted)]),
        )
        .unwrap();
        assert_eq!(params["count"].to_string(), accepted);
    }
}

#[test]
fn parameter_codec_raw_json_rejects_duplicates_and_validates_known_fields() {
    let registry = ManifestRegistry::embedded().unwrap();
    let order_book = operation(&registry, "public get-order-book");

    let value = resolve(
        order_book,
        ParameterInput::Inline(
            r#"{"instrument_name":"BTC-PERPETUAL","depth":5,"future_key":{"x":1}}"#.into(),
        ),
    )
    .unwrap();
    assert_eq!(value["future_key"], json!({"x":1}));

    let directory = tempfile::tempdir().unwrap();
    let params_file = directory.path().join("params.json");
    fs::write(
        &params_file,
        br#"{"instrument_name":"BTC-PERPETUAL","depth":10}"#,
    )
    .unwrap();
    let from_file = resolve(order_book, ParameterInput::File(params_file)).unwrap();
    assert_eq!(
        from_file,
        json!({"instrument_name":"BTC-PERPETUAL","depth":10})
    );
    let missing_file = resolve(
        order_book,
        ParameterInput::File(directory.path().join("missing.json")),
    )
    .unwrap_err();
    assert_eq!(missing_file.class(), ErrorClass::LocalIo);

    for payload in [
        r#"{"instrument_name":"BTC","instrument_name":"ETH"}"#,
        r#"{"instrument_name":"BTC","future":{"x":1,"x":2}}"#,
        r#"{"instrument_name":"BTC","depth":"5"}"#,
        r#"{"instrument_name":"BTC","depth":2}"#,
        r#"[]"#,
        r#"null"#,
        r#"{} {}"#,
        r#""#,
    ] {
        let error = resolve(order_book, ParameterInput::Inline(payload.into())).unwrap_err();
        assert_eq!(error.class(), ErrorClass::Input, "payload={payload:?}");
    }

    let missing = resolve(order_book, ParameterInput::Inline(r#"{"depth":5}"#.into())).unwrap_err();
    assert_eq!(missing.class(), ErrorClass::Usage);

    let constrained = resolve(
        operation(&registry, "public get-announcements"),
        ParameterInput::Inline(r#"{"count":999999999999999999999999999999}"#.into()),
    )
    .unwrap_err();
    assert_eq!(constrained.class(), ErrorClass::Input);

    let oversized_integer = resolve(
        operation(&registry, "public get-order-book-by-instrument-id"),
        ParameterInput::Inline(
            r#"{"instrument_id":999999999999999999999999999999999999999}"#.into(),
        ),
    )
    .unwrap();
    assert_eq!(
        oversized_integer["instrument_id"].to_string(),
        "999999999999999999999999999999999999999"
    );
}

#[test]
fn parameter_codec_enforces_bom_utf8_depth_and_byte_budget() {
    let registry = ManifestRegistry::embedded().unwrap();
    let get_time = operation(&registry, "public get-time");

    let mut stdin = Cursor::new([b"\xef\xbb\xbf".as_slice(), br#"{}"#].concat());
    let value = resolve_parameters(get_time, ParameterInput::Stdin, &mut stdin, false).unwrap();
    assert_eq!(value, json!({}));

    let mut terminal = Cursor::new(br#"{}"#);
    let error =
        resolve_parameters(get_time, ParameterInput::Stdin, &mut terminal, true).unwrap_err();
    assert_eq!(error.class(), ErrorClass::Input);

    let mut invalid_utf8 = Cursor::new(vec![0xff, b'{', b'}']);
    let error =
        resolve_parameters(get_time, ParameterInput::Stdin, &mut invalid_utf8, false).unwrap_err();
    assert_eq!(error.class(), ErrorClass::Input);

    for bytes in [
        Vec::new(),
        b" \n\t".to_vec(),
        b"{}\n{}".to_vec(),
        vec![0xff, 0xfe, b'{', 0, b'}', 0],
        vec![0xfe, 0xff, 0, b'{', 0, b'}'],
    ] {
        let mut stdin = Cursor::new(bytes);
        let error =
            resolve_parameters(get_time, ParameterInput::Stdin, &mut stdin, false).unwrap_err();
        assert_eq!(error.class(), ErrorClass::Input);
    }

    let inline_bom = String::from_utf8([b"\xef\xbb\xbf".as_slice(), br#"{}"#].concat()).unwrap();
    assert!(resolve(get_time, ParameterInput::Inline(inline_bom)).is_err());

    let depth_64 = format!("{}0{}", "[".repeat(63), "]".repeat(63));
    let accepted = format!(r#"{{"future":{depth_64}}}"#);
    resolve(get_time, ParameterInput::Inline(accepted)).unwrap();
    let depth_65 = format!("{}0{}", "[".repeat(64), "]".repeat(64));
    let rejected = format!(r#"{{"future":{depth_65}}}"#);
    assert!(resolve(get_time, ParameterInput::Inline(rejected)).is_err());

    let exact = format!(r#"{{"future":"{}"}}"#, "x".repeat(1_048_563));
    assert_eq!(exact.len(), 1_048_576);
    resolve(get_time, ParameterInput::Inline(exact)).unwrap();
    let mut exact_stdin = Cursor::new(format!(r#"{{"future":"{}"}}"#, "x".repeat(1_048_563)));
    resolve_parameters(get_time, ParameterInput::Stdin, &mut exact_stdin, false).unwrap();
    let over = format!(r#"{{"future":"{}"}}"#, "x".repeat(1_048_564));
    assert_eq!(over.len(), 1_048_577);
    assert!(resolve(get_time, ParameterInput::Inline(over)).is_err());
    let mut over_stdin = Cursor::new(format!(r#"{{"future":"{}"}}"#, "x".repeat(1_048_564)));
    let error =
        resolve_parameters(get_time, ParameterInput::Stdin, &mut over_stdin, false).unwrap_err();
    assert_eq!(error.class(), ErrorClass::Input);
}

#[test]
fn parameter_codec_cli_adapter_binds_only_canonical_commands() {
    let registry = ManifestRegistry::embedded().unwrap();
    let parsed = parse_from_with_stdin(
        &registry,
        [
            "deribit-cli",
            "--env",
            "testnet",
            "--timeout",
            "250ms",
            "public",
            "get-order-book",
            "--instrument-name",
            "BTC-PERPETUAL",
            "--depth",
            "5",
        ],
        &mut Cursor::new(Vec::new()),
        false,
    )
    .unwrap();
    let CliCommand::Public(public) = parsed.command else {
        panic!("expected public invocation")
    };
    assert_eq!(public.method, "public/get_order_book");
    assert_eq!(public.operation_id, "public_get_order_book");
    assert_eq!(
        public.params,
        json!({"instrument_name":"BTC-PERPETUAL","depth":5})
    );
    assert_eq!(public.timeout.as_millis(), 250);
    assert_eq!(public.environment, deribit_cli::Environment::Testnet);

    let global_separator = parse_from_with_stdin(
        &registry,
        ["deribit-cli", "--", "public", "get-time"],
        &mut Cursor::new(Vec::new()),
        false,
    )
    .unwrap();
    assert!(matches!(global_separator.command, CliCommand::Public(_)));
    let trailing_separator = parse_from_with_stdin(
        &registry,
        ["deribit-cli", "public", "get-time", "--"],
        &mut Cursor::new(Vec::new()),
        false,
    )
    .unwrap();
    assert!(matches!(trailing_separator.command, CliCommand::Public(_)));

    for args in [
        vec!["deribit-cli", "public", "get_order_book"],
        vec!["deribit-cli", "public", "get-ord"],
        vec!["deribit-cli", "private", "get-order-book"],
        vec![
            "deribit-cli",
            "public",
            "get-order-book",
            "--env",
            "testnet",
        ],
        vec!["deribit-cli", "--timeout", "0s", "public", "get-time"],
        vec!["deribit-cli", "--", "public", "get-time", "--"],
        vec!["deribit-cli", "--help", "--env", "testnet"],
        vec!["deribit-cli", "--version", "public", "get-time"],
    ] {
        let error = parse_from_with_stdin(&registry, args, &mut Cursor::new(Vec::new()), false)
            .unwrap_err();
        assert_eq!(error.class(), Some(ErrorClass::Usage));
    }

    let conflict = parse_from_with_stdin(
        &registry,
        [
            "deribit-cli",
            "--params",
            r#"{"instrument_name":"BTC-PERPETUAL"}"#,
            "public",
            "get-order-book",
            "--instrument-name",
            "BTC-PERPETUAL",
        ],
        &mut Cursor::new(Vec::new()),
        false,
    )
    .unwrap_err();
    assert_eq!(conflict.class(), Some(ErrorClass::Usage));
}
