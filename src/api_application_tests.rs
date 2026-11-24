use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::duplex;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

use super::{
    Authentication, Call, Catalog, Environment, Failure, OutputMode, Scope, Session, Step,
    build_steps, command, is_mainnet_action, run_steps,
};

const FAKE_ACCESS: &str = "fake-main-access";
const FAKE_REFRESH: &str = "fake-main-refresh";

fn call(method: &str, params: Value) -> Call {
    serde_json::from_value(json!({"method":method, "params":params})).unwrap()
}

#[test]
fn explicit_scope_cannot_be_widened_by_session_input_or_server() {
    let catalog = Catalog::load();
    let scope = Scope::request("connection trade:read").unwrap();
    for invalid in [
        "",
        "account:read",
        "session:prod connection",
        "connection trade:read trade:read_write",
    ] {
        assert!(Scope::request(invalid).is_err());
    }
    let mut auth = call(
        "public/auth",
        json!({"scope":"connection trade:read_write"}),
    );
    assert!(scope.prepare(&mut auth, &catalog).is_err());
    let mut mov = call(
        "private/move_positions",
        json!({"source_uid":1,"target_uid":2}),
    );
    assert!(is_mainnet_action(&mov, &catalog));
    assert!(scope.prepare(&mut mov, &catalog).is_err());
    let scope = Scope::request("connection trade:read_write").unwrap();
    assert!(scope.prepare(&mut mov, &catalog).is_ok());
    assert!(
        scope
            .check(&mov, &catalog, Some("connection trade:read_write"))
            .is_err()
    );
    assert!(
        scope
            .check(&mov, &catalog, Some("connection mainaccount trade:read"))
            .is_err()
    );
    assert!(
        scope
            .check(
                &mov,
                &catalog,
                Some("connection mainaccount trade:read_write")
            )
            .is_ok()
    );
    let mut mmp = call("private/get_mmp_status", json!({"block_rfq":true}));
    assert!(scope.prepare(&mut mmp, &catalog).is_err());
    assert!(
        Scope::request("connection block_rfq:read")
            .unwrap()
            .prepare(&mut mmp, &catalog)
            .is_ok()
    );
    let mut positions = call("private/get_positions", json!({"currency":"any"}));
    let scope = Scope::request("connection trade:read").unwrap();
    assert!(!is_mainnet_action(&positions, &catalog));
    assert!(scope.prepare(&mut positions, &catalog).is_ok());
    assert!(
        scope
            .check(&positions, &catalog, Some("connection trade:read"))
            .is_ok()
    );
    assert!(
        scope
            .check(&positions, &catalog, Some("connection trade:none"))
            .is_err()
    );
    let mut sub = call(
        "private/subscribe",
        json!({"channels":["block_rfq.maker.BTC"]}),
    );
    assert!(scope.prepare(&mut sub, &catalog).is_err());
}

#[test]
fn named_and_raw_commands_build_the_same_typed_requests() {
    let registry = crate::ManifestRegistry::embedded().unwrap();
    let catalog = Catalog::load();
    for (named, method, params, effect_needs_auth) in [
        (
            &[
                "market",
                "order-book",
                "--instrument-name",
                "BTC-PERPETUAL",
                "--depth",
                "5",
            ][..],
            "public/get_order_book",
            json!({"instrument_name": "BTC-PERPETUAL", "depth": 5}),
            false,
        ),
        (
            &["portfolio", "positions", "--currency", "BTC"][..],
            "private/get_positions",
            json!({"currency": "BTC"}),
            true,
        ),
    ] {
        let file = tempfile::NamedTempFile::new().unwrap();
        serde_json::to_writer(file.as_file(), &params).unwrap();
        let raw = [
            "rpc",
            "call",
            method,
            "--params-file",
            file.path().to_str().unwrap(),
        ];
        for args in [named, &raw] {
            let matches = command(&registry)
                .try_get_matches_from(
                    ["deribit-cli", "--env", "testnet"]
                        .into_iter()
                        .chain(args.iter().copied()),
                )
                .unwrap();
            let (group, args) = matches.subcommand().unwrap();
            let (steps, mode, continue_read_errors) =
                build_steps(&registry, &catalog, group, args).unwrap();
            assert!(mode == OutputMode::Response && !continue_read_errors);
            let [Step::Rpc(plan)] = steps.as_slice() else {
                panic!("expected one request")
            };
            assert_eq!(plan.method, method);
            assert_eq!(Value::Object(plan.params.clone()), params);
            assert_eq!(
                catalog.method(&plan.method).unwrap().auth,
                effect_needs_auth
            );
        }
    }
}

#[test]
fn mainnet_dispatch_uses_actual_grant_and_halts_on_profile_or_write_failure() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        for (granted, auth_marker, move_marker, expected, mode) in [
            ("connection trade:read_write", false, false, "scope", OutputMode::Stream),
            ("connection mainaccount trade:read_write", true, false, "profile", OutputMode::Stream),
            ("connection mainaccount trade:read_write", false, true, "uncertain", OutputMode::Stream),
            ("connection mainaccount trade:read_write", false, false, "success", OutputMode::Stream),
            ("connection mainaccount trade:read_write", false, false, "success", OutputMode::Response),
        ] {
            let (client, server) = duplex(32 * 1024);
            let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
            let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
            let server = tokio::spawn(async move {
                let request: Value = serde_json::from_str(&server.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
                assert_eq!(request["method"], "public/auth");
                assert_eq!(request["params"]["scope"], "connection trade:read_write account:none wallet:none custody:none block_trade:none block_rfq:none");
                let result = json!({"access_token":FAKE_ACCESS,"refresh_token":FAKE_REFRESH,"scope":granted});
                server.send(Message::Text(json!({"jsonrpc":"2.0","id":request["id"],"testnet":auth_marker,"result":result}).to_string().into())).await.unwrap();
                if matches!(expected, "uncertain" | "success") {
                    let request: Value = serde_json::from_str(&server.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
                    assert_eq!(request["method"], "private/move_positions");
                    assert_eq!(request["params"]["access_token"], FAKE_ACCESS);
                    assert_eq!(request["params"]["trades"][0]["amount"], 10);
                    server.send(Message::Text(json!({"jsonrpc":"2.0","id":request["id"],"testnet":move_marker,"result":{"trades":[]}}).to_string().into())).await.unwrap();
                }
                assert!(!matches!(server.next().await, Some(Ok(Message::Text(_)))));
            });
            let catalog = Catalog::load();
            let mut mov = call("private/move_positions", json!({
                "source_uid":1,"target_uid":2,"trades":[{"instrument_name":"BTC-PERPETUAL","amount":10}]
            }));
            let scope = Scope::request("connection trade:read_write").unwrap();
            scope.prepare(&mut mov,&catalog).unwrap();
            let session = Session::fixture(client, Environment::Mainnet);
            let mut out = Vec::new();
            let result = run_steps(session, &catalog, &Authentication {
                enabled: true, requested: None, mainnet_scope: Some(scope)
            }, vec![Step::Rpc(mov)], mode, false, &mut out).await;
            assert!(matches!((expected,result), ("scope",Err(Failure::InsufficientScope))
                | ("profile",Err(Failure::Protocol)) | ("uncertain",Err(Failure::UncertainWrite))
                | ("success",Ok(()))));
            let out = String::from_utf8(out).unwrap();
            assert!(!out.contains("fake-main-"));
            let lines = match (expected, mode) {
                ("profile", _) => 0,
                ("success", OutputMode::Stream) => 2,
                _ => 1,
            };
            assert_eq!(out.lines().count(), lines);
            if mode == OutputMode::Response {
                let response: Value = serde_json::from_str(&out).unwrap();
                assert_eq!(response["result"], json!({"trades": []}));
            }
            server.await.unwrap();
        }
    });
}
