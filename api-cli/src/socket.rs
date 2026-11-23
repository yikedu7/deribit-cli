use std::env;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_lc_rs::hmac;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::{Instant, timeout_at};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, client_async_tls_with_config};

use crate::application::{Failure, Profile, write_line};
use deribit_cli::runtime::RequestId;

pub(crate) struct Credentials {
    profile: Profile,
    id: String,
    secret: String,
}

impl Credentials {
    pub fn load(
        profile: Profile,
        mount: Option<&Path>,
        id: Option<&str>,
        secret: Option<&str>,
    ) -> Result<Self, Failure> {
        let (id, secret) = selected_variables(profile, id, secret)?;
        if profile == Profile::Mainnet && mount.is_some() {
            return Err(Failure::Input(
                "Mainnet credentials require the dedicated Mainnet environment variables",
            ));
        }
        let (id, secret) = if let Some(mount) = mount {
            Self::from_mount(mount, id, secret)?
        } else {
            (
                env::var(id).map_err(|_| Failure::Credentials)?,
                env::var(secret).map_err(|_| Failure::Credentials)?,
            )
        };
        if id.trim().is_empty() || secret.trim().is_empty() {
            return Err(Failure::Credentials);
        }
        Ok(Self {
            profile,
            id,
            secret,
        })
    }

    #[cfg(unix)]
    fn from_mount(path: &Path, id: &str, secret: &str) -> Result<(String, String), Failure> {
        use std::fs::File;
        use std::io::{BufReader, Read};
        use std::os::unix::fs::{FileTypeExt, MetadataExt};

        let invalid_mount = || {
            Failure::Input("credential mount must be an existing absolute, non-symlink FIFO path")
        };
        if !path.is_absolute() {
            return Err(invalid_mount());
        }
        let expected = std::fs::symlink_metadata(path).map_err(|_| invalid_mount())?;
        if !expected.file_type().is_fifo() {
            return Err(invalid_mount());
        }
        let file = File::open(path).map_err(|_| Failure::Credentials)?;
        let opened = file.metadata().map_err(|_| Failure::Credentials)?;
        if !opened.file_type().is_fifo()
            || (opened.dev(), opened.ino()) != (expected.dev(), expected.ino())
        {
            return Err(Failure::Credentials);
        }
        literal_mount_credentials(BufReader::new(file.take(64 * 1024)), id, secret)
    }

    #[cfg(not(unix))]
    fn from_mount(_path: &Path, _id: &str, _secret: &str) -> Result<(String, String), Failure> {
        Err(Failure::Input(
            "1Password FIFO mounts require macOS or Linux",
        ))
    }
}

#[cfg(unix)]
fn literal_mount_credentials(
    reader: impl std::io::BufRead,
    id: &str,
    secret: &str,
) -> Result<(String, String), Failure> {
    let mut client_id = None;
    let mut client_secret = None;
    for line in reader.lines() {
        let line = line.map_err(|_| Failure::Credentials)?;
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let slot = match key.trim() {
            key if key == id => &mut client_id,
            key if key == secret => &mut client_secret,
            _ => continue,
        };
        let value = raw.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|text| text.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|text| text.strip_suffix('\''))
            })
            .unwrap_or(value);
        // Deribit keys are opaque literal tokens. Never resolve dotenv
        // interpolation, which can read a different profile's environment.
        if slot.is_some()
            || value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-./+=".contains(&byte))
        {
            return Err(Failure::Credentials);
        }
        *slot = Some(value.to_owned());
        if client_id.is_some() && client_secret.is_some() {
            break;
        }
    }
    client_id.zip(client_secret).ok_or(Failure::Credentials)
}

fn selected_variables(
    profile: Profile,
    id: Option<&str>,
    secret: Option<&str>,
) -> Result<(&'static str, &'static str), Failure> {
    const MAINNET: (&str, &str) = ("DERIBIT_MAINNET_CLIENT_ID", "DERIBIT_MAINNET_CLIENT_SECRET");
    const TESTNET: (&str, &str) = ("DERIBIT_TESTNET_CLIENT_ID", "DERIBIT_TESTNET_CLIENT_SECRET");
    const LEGACY_TESTNET: (&str, &str) = ("DERIBIT_TEST_CLIENT_ID", "DERIBIT_TEST_CLIENT_SECRET");
    match profile {
        Profile::Mainnet if id.is_none() && secret.is_none() => Ok(MAINNET),
        Profile::Mainnet => Err(Failure::Input(
            "Mainnet credentials require the dedicated Mainnet environment variables",
        )),
        Profile::Testnet => {
            let pair = (id.unwrap_or(TESTNET.0), secret.unwrap_or(TESTNET.1));
            [TESTNET, LEGACY_TESTNET]
                .into_iter()
                .find(|allowed| *allowed == pair)
                .ok_or(Failure::Input(
                    "select a matching pair of Testnet credential variables",
                ))
        }
    }
}

pub(crate) struct Session<S> {
    stream: WebSocketStream<S>,
    profile: Profile,
    timeout: Duration,
    credentials: Option<Credentials>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    granted_scope: Option<String>,
    block_trade_signature: Option<String>,
    heartbeat_id: Option<String>,
}

impl Session<MaybeTlsStream<TcpStream>> {
    pub async fn connect(
        profile: Profile,
        timeout: Duration,
        credentials: Option<Credentials>,
    ) -> Result<Self, Failure> {
        if credentials
            .as_ref()
            .is_some_and(|pair| pair.profile != profile)
        {
            return Err(Failure::Credentials);
        }
        let stream = timeout_at(Instant::now() + timeout, async {
            let request = profile
                .endpoint()
                .into_client_request()
                .map_err(|_| Failure::Connect)?;
            let host = request.uri().host().ok_or(Failure::Connect)?;
            let socket = TcpStream::connect((host, 443))
                .await
                .map_err(|_| Failure::Connect)?;
            socket.set_nodelay(true).map_err(|_| Failure::Connect)?;
            handshake_once(request, socket).await
        })
        .await
        .map_err(|_| Failure::Timeout)??;
        Ok(Self::new(stream, profile, timeout, credentials))
    }
}

// A connection performs one handshake with the selected host. A redirect
// response ends the attempt without handing it to a second connection.
async fn handshake_once<S: AsyncRead + AsyncWrite + Send + Unpin + 'static>(
    request: Request,
    socket: S,
) -> Result<WebSocketStream<MaybeTlsStream<S>>, Failure> {
    let config = WebSocketConfig::default()
        .max_frame_size(Some(16 * 1024 * 1024))
        .max_message_size(Some(16 * 1024 * 1024))
        .max_write_buffer_size(2 * 1024 * 1024);
    client_async_tls_with_config(request, socket, Some(config), None)
        .await
        .map(|(stream, _)| stream)
        .map_err(|_| Failure::Connect)
}

impl<S: AsyncRead + AsyncWrite + Unpin> Session<S> {
    fn new(
        stream: WebSocketStream<S>,
        profile: Profile,
        timeout: Duration,
        credentials: Option<Credentials>,
    ) -> Self {
        Self {
            stream,
            profile,
            timeout,
            credentials,
            access_token: None,
            refresh_token: None,
            granted_scope: None,
            block_trade_signature: None,
            heartbeat_id: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture(stream: WebSocketStream<S>, profile: Profile) -> Self {
        Self::new(
            stream,
            profile,
            Duration::from_secs(2),
            Some(Credentials {
                profile,
                id: "synthetic-client-id".to_owned(),
                secret: "synthetic-client-secret".to_owned(),
            }),
        )
    }

    pub fn granted_scope(&self) -> Option<&str> {
        self.granted_scope.as_deref()
    }

    pub async fn authenticate(
        &mut self,
        mut params: Map<String, Value>,
        output: &mut impl Write,
    ) -> Result<Value, Failure> {
        let grant = params
            .remove("grant_type")
            .unwrap_or_else(|| json!("client_credentials"));
        let credentials = self.credentials.as_ref().ok_or(Failure::Credentials)?;
        match grant.as_str() {
            Some("client_credentials") => {
                params.insert("client_id".to_owned(), json!(credentials.id));
                params.insert("client_secret".to_owned(), json!(credentials.secret));
            }
            Some("client_signature") => {
                let timestamp = u64::try_from(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|_| Failure::Protocol)?
                        .as_millis(),
                )
                .map_err(|_| Failure::Protocol)?;
                let nonce = RequestId::new_v4().to_string();
                let data = params
                    .get("data")
                    .map(|value| {
                        value
                            .as_str()
                            .ok_or(Failure::Input("signature data must be a string"))
                    })
                    .transpose()?
                    .unwrap_or_default();
                let signature = sign(&credentials.secret, timestamp, &nonce, data);
                params.insert("client_id".to_owned(), json!(credentials.id));
                params.insert("timestamp".to_owned(), json!(timestamp));
                params.insert("nonce".to_owned(), json!(nonce));
                params.insert("signature".to_owned(), json!(signature));
            }
            Some("refresh_token") => self.insert_refresh_token(&mut params)?,
            _ => return Err(Failure::Input("unsupported authentication grant")),
        }
        params.insert("grant_type".to_owned(), grant);
        let deadline = Instant::now() + self.timeout;
        let response = self.exchange("public/auth", params, deadline, output).await;
        self.finish_authentication(response)
    }

    pub async fn call(
        &mut self,
        method: &str,
        mut params: Map<String, Value>,
        authenticated: bool,
        output: &mut impl Write,
    ) -> Result<Value, Failure> {
        let changes_token = matches!(method, "public/exchange_token" | "public/fork_token");
        if changes_token {
            self.insert_refresh_token(&mut params)?;
        } else if authenticated || (method.starts_with("public/") && self.access_token.is_some()) {
            let token = self.access_token.as_ref().ok_or(Failure::Credentials)?;
            params.insert("access_token".to_owned(), json!(token));
        }
        if method == "private/invalidate_block_trade_signature" && !params.contains_key("signature")
        {
            let signature = self.block_trade_signature.as_ref().ok_or(Failure::Input(
                "verify the Block Trade in this session before invalidating its signature",
            ))?;
            params.insert("signature".to_owned(), json!(signature));
        }
        let response = self
            .exchange(method, params, Instant::now() + self.timeout, output)
            .await;
        if changes_token {
            self.finish_authentication(response)
        } else {
            let response = response?;
            if method == "private/verify_block_trade" {
                if let Some(signature) = response["result"]["signature"]
                    .as_str()
                    .filter(|signature| !signature.is_empty())
                {
                    self.block_trade_signature = Some(signature.to_owned());
                }
            }
            Ok(self.redacted(response))
        }
    }

    fn insert_refresh_token(&self, params: &mut Map<String, Value>) -> Result<(), Failure> {
        let token = self.refresh_token.as_ref().ok_or(Failure::Input(
            "authenticate in this session before refreshing, exchanging or forking a token",
        ))?;
        params.insert("refresh_token".to_owned(), json!(token));
        Ok(())
    }

    fn accept_tokens(&mut self, mut response: Value) -> Result<Value, Failure> {
        if !self.profile.accepts(&response) {
            return Err(Failure::Protocol);
        }
        if let Some(result) = response.get("result") {
            let access_token = result
                .get("access_token")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .ok_or(Failure::Protocol)?
                .to_owned();
            let refresh_token = result
                .get("refresh_token")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let scope = result
                .get("scope")
                .and_then(Value::as_str)
                .map(str::to_owned);
            // Clear old session secrets from any echoed fields before rotation.
            response = self.redacted(response);
            self.access_token = Some(access_token);
            self.refresh_token = refresh_token;
            self.granted_scope = scope;
            self.block_trade_signature = None;
        } else {
            response = self.redacted(response);
            self.clear_tokens();
        }
        Ok(self.redacted(response))
    }

    fn finish_authentication(
        &mut self,
        response: Result<Value, Failure>,
    ) -> Result<Value, Failure> {
        let result = response.and_then(|response| self.accept_tokens(response));
        if result.is_err() {
            self.clear_tokens();
        }
        result
    }

    fn clear_tokens(&mut self) {
        self.access_token = None;
        self.refresh_token = None;
        self.granted_scope = None;
        self.block_trade_signature = None;
    }

    async fn exchange(
        &mut self,
        method: &str,
        params: Map<String, Value>,
        deadline: Instant,
        output: &mut impl Write,
    ) -> Result<Value, Failure> {
        let id = self.send(method, params, deadline).await?;
        loop {
            match self.receive(deadline).await? {
                Incoming::Response(response) if response["id"].as_str() == Some(&id) => {
                    return Ok(response);
                }
                Incoming::Response(_) => return Err(Failure::Protocol),
                Incoming::Notification(frame) => write_line(output, &self.redacted(frame))?,
                Incoming::Closed if method == "private/logout" => {
                    // Deribit specifies a WebSocket close with no RPC response.
                    return Ok(json!({"client_event": "websocket_closed", "method": method}));
                }
                Incoming::Closed => return Err(Failure::Connection),
            }
        }
    }

    pub async fn observe(
        &mut self,
        duration: Duration,
        output: &mut impl Write,
    ) -> Result<(), Failure> {
        let deadline = Instant::now() + duration;
        loop {
            match self.receive(deadline).await {
                Ok(Incoming::Notification(frame)) => write_line(output, &self.redacted(frame))?,
                Ok(Incoming::Response(_)) => return Err(Failure::Protocol),
                Ok(Incoming::Closed) => return Err(Failure::Connection),
                Err(Failure::Timeout) => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }

    async fn send(
        &mut self,
        method: &str,
        params: Map<String, Value>,
        deadline: Instant,
    ) -> Result<String, Failure> {
        let id = RequestId::new_v4().to_string();
        let envelope = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        timeout_at(
            deadline,
            self.stream.send(Message::Text(envelope.to_string().into())),
        )
        .await
        .map_err(|_| Failure::Timeout)?
        .map_err(|_| Failure::Connection)?;
        Ok(id)
    }

    async fn receive(&mut self, deadline: Instant) -> Result<Incoming, Failure> {
        loop {
            let message = timeout_at(deadline, self.stream.next())
                .await
                .map_err(|_| Failure::Timeout)?
                .ok_or(Failure::Connection)?
                .map_err(|_| Failure::Connection)?;
            let text = match message {
                Message::Text(text) => text,
                Message::Close(frame)
                    if frame
                        .as_ref()
                        .is_none_or(|frame| frame.code == CloseCode::Normal) =>
                {
                    return Ok(Incoming::Closed);
                }
                Message::Close(_) => return Err(Failure::Connection),
                Message::Ping(_) => {
                    timeout_at(deadline, self.stream.flush())
                        .await
                        .map_err(|_| Failure::Timeout)?
                        .map_err(|_| Failure::Connection)?;
                    continue;
                }
                Message::Pong(_) => continue,
                _ => return Err(Failure::Protocol),
            };
            let frame: Value = serde_json::from_str(&text).map_err(|_| Failure::Protocol)?;
            if frame["jsonrpc"] != "2.0" {
                return Err(Failure::Protocol);
            }
            if let Some(method) = frame["method"].as_str() {
                if frame.get("id").is_some()
                    || (frame.get("testnet").is_some() && !self.profile.accepts(&frame))
                {
                    return Err(Failure::Protocol);
                }
                if method == "heartbeat" && frame["params"]["type"] == "test_request" {
                    if self.heartbeat_id.is_some() {
                        return Err(Failure::Protocol);
                    }
                    self.heartbeat_id = Some(self.send("public/test", Map::new(), deadline).await?);
                }
                return Ok(Incoming::Notification(frame));
            }
            if frame["id"].as_str().is_none()
                || frame.get("result").is_some() == frame.get("error").is_some()
                || frame.get("error").is_some_and(|error| !error.is_object())
                || !self.profile.accepts(&frame)
            {
                return Err(Failure::Protocol);
            }
            if self.heartbeat_id.as_deref() == frame["id"].as_str() {
                self.heartbeat_id = None;
                if frame.get("error").is_some() {
                    return Err(Failure::Protocol);
                }
                continue;
            }
            return Ok(Incoming::Response(frame));
        }
    }

    fn redacted(&self, mut value: Value) -> Value {
        redact(
            &mut value,
            [
                self.credentials.as_ref().map(|auth| auth.id.as_str()),
                self.credentials.as_ref().map(|auth| auth.secret.as_str()),
                self.access_token.as_deref(),
                self.refresh_token.as_deref(),
                self.block_trade_signature.as_deref(),
            ],
        );
        value
    }

    pub async fn close(&mut self) {
        let _ = timeout_at(Instant::now() + self.timeout, self.stream.close(None)).await;
    }
}

enum Incoming {
    Response(Value),
    Notification(Value),
    Closed,
}

fn redact(value: &mut Value, credentials: [Option<&str>; 5]) {
    match value {
        Value::Object(object) => {
            for (key, field) in object {
                if matches!(
                    key.as_str(),
                    "access_token"
                        | "refresh_token"
                        | "client_id"
                        | "client_secret"
                        | "signature"
                        | "api_key_secret"
                        | "secret"
                        | "password"
                ) {
                    *field = Value::String("[REDACTED]".to_owned());
                } else {
                    redact(field, credentials);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact(item, credentials);
            }
        }
        Value::String(text) => {
            for secret in credentials
                .into_iter()
                .flatten()
                .filter(|secret| !secret.is_empty())
            {
                if text.contains(secret) {
                    *text = text.replace(secret, "[REDACTED]");
                }
            }
        }
        _ => {}
    }
}

fn sign(secret: &str, timestamp: u64, nonce: &str, data: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let message = format!("{timestamp}\n{nonce}\n{data}");
    let tag = hmac::sign(&key, message.as_bytes());
    let mut signature = String::with_capacity(tag.as_ref().len() * 2);
    let hex = b"0123456789abcdef";
    for byte in tag.as_ref() {
        signature.push(char::from(hex[usize::from(byte >> 4)]));
        signature.push(char::from(hex[usize::from(byte & 0x0f)]));
    }
    signature
}

#[cfg(test)]
mod tests {
    const SYNTHETIC_ACCESS: &str = "synthetic-access-token";
    const SYNTHETIC_REFRESH: &str = "synthetic-refresh-token";
    const ECHO_SECRET: &str = "another-secret";
    const DIFFERENT_ACCESS: &str = "another-token";
    const WRONG_PROFILE_ACCESS: &str = "wrong-profile-token";
    const OLD_ACCESS: &str = "old-access-token";
    const OLD_REFRESH: &str = "old-refresh-token";
    const FIXTURE_ACCESS: &str = "fixture-token";

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};
    use tokio_tungstenite::tungstenite::protocol::Role;

    #[cfg(unix)]
    #[test]
    fn reads_named_mount_variables_from_a_fifo() {
        use std::fs::OpenOptions;
        use std::process::Command;

        let directory = tempfile::tempdir().unwrap();
        let mount = directory.path().join("testnet.env");
        assert!(
            Command::new("mkfifo")
                .arg(&mount)
                .status()
                .unwrap()
                .success()
        );
        let writer_mount = mount.clone();
        let writer = std::thread::spawn(move || {
            let mut fifo = OpenOptions::new().write(true).open(writer_mount).unwrap();
            let (id, secret) = selected_variables(Profile::Testnet, None, None).unwrap();
            let fixture =
                format!("IGNORED=synthetic\n{id}='fixture-id'\n{secret}=\"fixture-secret\"\n");
            fifo.write_all(fixture.as_bytes()).unwrap();
        });
        let credentials = Credentials::load(Profile::Testnet, Some(&mount), None, None).unwrap();
        assert_eq!(credentials.id, "fixture-id");
        assert_eq!(credentials.secret, "fixture-secret");
        writer.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fifo_never_expands_variables_from_another_profile() {
        let (id, secret) = selected_variables(Profile::Testnet, None, None).unwrap();
        let (_, mainnet_secret) = selected_variables(Profile::Mainnet, None, None).unwrap();
        let fixture = format!("{id}=fixture-id\n{secret}=\"${{{mainnet_secret}}}\"\n");
        assert!(matches!(
            literal_mount_credentials(fixture.as_bytes(), id, secret),
            Err(Failure::Credentials)
        ));
    }

    #[test]
    fn private_chain_handles_heartbeat_and_redacts_credentials() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (client, server) = duplex(8192);
            let server =
                tokio::spawn(async move {
                    let mut server =
                        WebSocketStream::from_raw_socket(server, Role::Server, None).await;
                    let auth = next_request(&mut server).await;
                    assert_eq!(auth["method"], "public/auth");
                    assert_eq!(auth["params"]["client_secret"], "synthetic-client-secret");
                    reply(
                        &mut server,
                        &auth,
                        json!({
                            "access_token": SYNTHETIC_ACCESS,
                            "refresh_token": SYNTHETIC_REFRESH,
                            "scope": "connection account:read",
                        }),
                    )
                    .await;

                    let request = next_request(&mut server).await;
                    assert_eq!(request["method"], "private/get_account_summary");
                    assert_eq!(request["params"]["access_token"], SYNTHETIC_ACCESS);
                    server.send(Message::Text(json!({
                    "jsonrpc":"2.0","method":"heartbeat","params":{"type":"test_request"}
                }).to_string().into())).await.unwrap();
                    let heartbeat = next_request(&mut server).await;
                    assert_eq!(heartbeat["method"], "public/test");
                    reply(&mut server, &heartbeat, json!({"version":"fixture"})).await;
                    reply(
                        &mut server,
                        &request,
                        json!({
                            "balance":0, "echo":"synthetic-client-secret",
                            "client_secret":ECHO_SECRET,
                        }),
                    )
                    .await;
                });

            let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
            let mut session = Session::new(
                client,
                Profile::Testnet,
                Duration::from_secs(2),
                Some(Credentials {
                    profile: Profile::Testnet,
                    id: "synthetic-client-id".to_owned(),
                    secret: "synthetic-client-secret".to_owned(),
                }),
            );
            let mut notifications = Vec::new();
            let reply = session
                .authenticate(Map::new(), &mut notifications)
                .await
                .unwrap();
            assert_eq!(reply["result"]["scope"], "connection account:read");
            assert_eq!(reply["result"]["access_token"], "[REDACTED]");
            assert_eq!(reply["result"]["refresh_token"], "[REDACTED]");

            let private = session
                .call(
                    "private/get_account_summary",
                    Map::new(),
                    true,
                    &mut notifications,
                )
                .await
                .unwrap();
            assert_eq!(private["result"]["balance"], 0);
            assert_eq!(private["result"]["echo"], "[REDACTED]");
            assert_eq!(private["result"]["client_secret"], "[REDACTED]");
            let notification: Value = serde_json::from_slice(&notifications).unwrap();
            assert_eq!(notification["method"], "heartbeat");
            server.await.unwrap();
        });
    }

    #[test]
    fn rejects_unrelated_response_id() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (client, server) = duplex(8192);
            let server = tokio::spawn(async move {
                let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
                let _ = next_request(&mut server).await;
                reply(&mut server, &json!({"id":"unrelated"}), json!(0)).await;
            });
            let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
            let mut session = Session::new(client, Profile::Testnet, Duration::from_secs(2), None);
            let result = session
                .call("public/get_time", Map::new(), false, &mut Vec::new())
                .await;
            assert!(matches!(result, Err(Failure::Protocol)));
            server.await.unwrap();
        });
    }

    #[test]
    fn disconnect_after_a_write_requires_reconciliation() {
        use crate::{application::uncertain_write, catalog::Effect};

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (client, server) = duplex(8192);
            let server = tokio::spawn(async move {
                let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
                let request = next_request(&mut server).await;
                assert_eq!(request["method"], "private/edit_by_label");
                assert_eq!(request["params"]["label"], "synthetic-owned-test-order");
                assert_eq!(request["params"]["access_token"], SYNTHETIC_ACCESS);
                server.close(None).await.unwrap();
            });
            let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
            let mut session = Session::new(client, Profile::Testnet, Duration::from_secs(2), None);
            session.access_token = Some(SYNTHETIC_ACCESS.to_owned());
            let params =
                Map::from_iter([("label".to_owned(), json!("synthetic-owned-test-order"))]);
            let result = session
                .call("private/edit_by_label", params, true, &mut Vec::new())
                .await
                .map_err(|error| uncertain_write(Effect::Write, error));
            assert!(matches!(result, Err(Failure::UncertainWrite)));
            server.await.unwrap();
        });
    }

    #[test]
    fn block_trade_signature_stays_in_its_testnet_token_chain() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (client, server) = duplex(8192);
            let server = tokio::spawn(async move {
                let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
                let verify = next_request(&mut server).await;
                assert_eq!(verify["method"], "private/verify_block_trade");
                server
                    .send(Message::Text(
                        json!({
                            "jsonrpc": "2.0", "id": verify["id"], "testnet": true,
                            "result": {"signature": "synthetic-signature"},
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();

                let invalidate = next_request(&mut server).await;
                assert_eq!(
                    invalidate["method"],
                    "private/invalidate_block_trade_signature"
                );
                assert_eq!(invalidate["params"]["signature"], "synthetic-signature");
                reply(
                    &mut server,
                    &invalidate,
                    json!({"echo": "synthetic-signature"}),
                )
                .await;
            });
            let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
            let mut session = Session::new(client, Profile::Testnet, Duration::from_secs(2), None);
            session.access_token = Some("synthetic-token".to_owned());
            let mut output = Vec::new();
            let verify = session
                .call("private/verify_block_trade", Map::new(), true, &mut output)
                .await
                .unwrap();
            assert_eq!(verify["result"]["signature"], "[REDACTED]");
            let invalidate = session
                .call(
                    "private/invalidate_block_trade_signature",
                    Map::new(),
                    true,
                    &mut output,
                )
                .await
                .unwrap();
            assert_eq!(invalidate["result"]["echo"], "[REDACTED]");

            session
                .accept_tokens(
                    json!({"testnet": true, "result": {"access_token": DIFFERENT_ACCESS}}),
                )
                .unwrap();
            let invalid = session
                .call(
                    "private/invalidate_block_trade_signature",
                    Map::new(),
                    true,
                    &mut output,
                )
                .await;
            assert!(matches!(invalid, Err(Failure::Input(_))));
            server.await.unwrap();
        });
    }

    #[test]
    fn logout_accepts_a_graceful_close_without_an_rpc_result() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (client, server) = duplex(8192);
            let server = tokio::spawn(async move {
                let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
                let request = next_request(&mut server).await;
                assert_eq!(request["method"], "private/logout");
                assert_eq!(request["params"]["invalidate_token"], false);
                server.close(None).await.unwrap();
            });
            let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
            let mut session = Session::new(client, Profile::Testnet, Duration::from_secs(2), None);
            session.access_token = Some(SYNTHETIC_ACCESS.to_owned());
            let params = Map::from_iter([("invalidate_token".to_owned(), json!(false))]);
            let result = session
                .call("private/logout", params, true, &mut Vec::new())
                .await
                .unwrap();
            assert_eq!(
                result,
                json!({"client_event": "websocket_closed", "method": "private/logout"})
            );
            server.await.unwrap();
        });
    }

    #[test]
    fn recorded_error_codes_remain_native_and_are_not_retried() {
        const CASES: [(&str, i32); 6] = [
            ("private/get_ctn_id", 13021),
            ("private/get_lsp_participant_config", -32602),
            ("private/get_lsp_usage", -32602),
            ("private/get_mmp_config", 11050),
            ("private/get_mmp_status", 11050),
            ("private/simulate_portfolio", 10028),
        ];
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (client, server) = duplex(8192);
            let server = tokio::spawn(async move {
                let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
                for (method, code) in CASES {
                    let request = next_request(&mut server).await;
                    assert_eq!(request["method"], method);
                    server
                        .send(Message::Text(
                            json!({"jsonrpc": "2.0", "id": request["id"], "testnet": true,
                                   "error": {"code": code, "message": "synthetic remote error"}})
                            .to_string()
                            .into(),
                        ))
                        .await
                        .unwrap();
                }
                assert!(
                    timeout_at(Instant::now() + Duration::from_millis(15), server.next())
                        .await
                        .is_err()
                );
            });
            let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
            let mut session = Session::new(client, Profile::Testnet, Duration::from_secs(2), None);
            session.access_token = Some("synthetic-token".to_owned());
            for (method, code) in CASES {
                let response = session
                    .call(method, Map::new(), true, &mut Vec::new())
                    .await
                    .unwrap();
                assert_eq!(response["error"]["code"], code);
                assert_eq!(response["testnet"], true);
            }
            server.await.unwrap();
        });
    }

    #[test]
    fn credentials_select_only_the_named_profile_and_reject_mixed_pairs() {
        let legacy_id = "DERIBIT_TEST_CLIENT_ID";
        let legacy_secret = "DERIBIT_TEST_CLIENT_SECRET";
        let main_id = "DERIBIT_MAINNET_CLIENT_ID";
        let main_secret = "DERIBIT_MAINNET_CLIENT_SECRET";
        assert_eq!(
            selected_variables(Profile::Mainnet, None, None).unwrap(),
            (main_id, main_secret)
        );
        assert_eq!(
            selected_variables(Profile::Testnet, Some(legacy_id), Some(legacy_secret)).unwrap(),
            (legacy_id, legacy_secret)
        );
        assert!(selected_variables(Profile::Testnet, Some(legacy_id), None).is_err());
        assert!(selected_variables(Profile::Testnet, Some(main_id), Some(main_secret)).is_err());
        assert!(
            selected_variables(Profile::Mainnet, Some(legacy_id), Some(legacy_secret)).is_err()
        );
        assert!(matches!(
            Credentials::load(Profile::Mainnet, Some(Path::new("/unused")), None, None),
            Err(Failure::Input(_))
        ));
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                let result = Session::connect(
                    Profile::Mainnet,
                    Duration::ZERO,
                    Some(fixture_credentials(Profile::Testnet)),
                )
                .await;
                assert!(matches!(result, Err(Failure::Credentials)));
            });
    }

    #[test]
    fn one_handshake_rejects_redirects_without_sending_rpc() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (client, mut server) = duplex(4096);
            let server = tokio::spawn(async move {
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") {
                    headers.push(server.read_u8().await.unwrap());
                }
                let headers = String::from_utf8(headers).unwrap();
                assert!(headers.starts_with("GET /ws/api/v2 HTTP/1.1\r\n"));
                assert!(!headers.contains("public/auth"));
                server.write_all(b"HTTP/1.1 302 Found\r\nLocation: ws://example.invalid/ws/api/v2\r\nContent-Length: 0\r\n\r\n").await.unwrap();
                assert_eq!(server.read(&mut [0]).await.unwrap(), 0);
            });
            // Only this fixture uses a plain handshake on an in-memory stream.
            let request = "ws://fixture.invalid/ws/api/v2".into_client_request().unwrap();
            assert!(matches!(handshake_once(request, client).await, Err(Failure::Connect)));
            server.await.unwrap();
        });
    }

    #[test]
    fn rejects_wrong_environment_before_caching_tokens_or_attempting_a_move() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for profile in [Profile::Testnet, Profile::Mainnet] {
                let (client, server) = duplex(8192);
                let server = tokio::spawn(async move {
                    let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
                    let request = next_request(&mut server).await;
                    assert_eq!(request["method"], "public/auth");
                    reply_with_marker(
                        &mut server, &request,
                        json!({"access_token":WRONG_PROFILE_ACCESS,"scope":"mainaccount trade:read_write"}),
                        profile == Profile::Mainnet,
                    ).await;
                    assert!(timeout_at(Instant::now() + Duration::from_millis(15), server.next()).await.is_err());
                });
                let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
                let mut session = Session::new(
                    client, profile, Duration::from_secs(2), Some(fixture_credentials(profile)),
                );
                let mut output = Vec::new();
                assert!(matches!(session.authenticate(Map::new(), &mut output).await, Err(Failure::Protocol)));
                assert!(session.granted_scope().is_none());
                assert!(matches!(
                    session.call("private/move_positions", Map::new(), true, &mut output).await,
                    Err(Failure::Credentials)
                ));
                assert!(output.is_empty());
                server.await.unwrap();
            }
        });
    }

    #[test]
    fn failed_refresh_or_wrong_profile_clears_the_previous_token_scope() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for marker in [true, false] {
                let (client, server) = duplex(8192);
                let server = tokio::spawn(async move {
                    let mut server =
                        WebSocketStream::from_raw_socket(server, Role::Server, None).await;
                    let request = next_request(&mut server).await;
                    assert_eq!(request["params"]["refresh_token"], OLD_REFRESH);
                    server
                        .send(Message::Text(
                            json!({
                                "jsonrpc": "2.0", "id": request["id"], "testnet": marker,
                                "error": {"code": 13009, "message": "echo old-access-token"},
                            })
                            .to_string()
                            .into(),
                        ))
                        .await
                        .unwrap();
                    assert!(
                        timeout_at(Instant::now() + Duration::from_millis(15), server.next())
                            .await
                            .is_err()
                    );
                });
                let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
                let mut session = Session::new(
                    client,
                    Profile::Testnet,
                    Duration::from_secs(2),
                    Some(fixture_credentials(Profile::Testnet)),
                );
                session
                    .accept_tokens(json!({"testnet": true, "result": {
                        "access_token":OLD_ACCESS,
                        "refresh_token":OLD_REFRESH,
                        "scope":"trade:read_write",
                    }}))
                    .unwrap();
                let mut output = Vec::new();
                let params = Map::from_iter([("grant_type".to_owned(), json!("refresh_token"))]);
                let result = session.authenticate(params, &mut output).await;
                if marker {
                    assert_eq!(result.unwrap()["error"]["message"], "echo [REDACTED]");
                } else {
                    assert!(matches!(result, Err(Failure::Protocol)));
                }
                assert!(session.granted_scope().is_none());
                assert!(session.access_token.is_none() && session.refresh_token.is_none());
                assert!(output.is_empty());
                server.await.unwrap();
            }
        });
    }

    #[test]
    fn both_profiles_serialize_move_positions_and_reject_opposite_write_reply() {
        use crate::{application::uncertain_write, catalog::Effect};

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for profile in [Profile::Testnet, Profile::Mainnet] {
                let (client, server) = duplex(8192);
                let marker = profile == Profile::Testnet;
                let server = tokio::spawn(async move {
                    let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
                    let auth = next_request(&mut server).await;
                    assert_eq!(auth["method"], "public/auth");
                    reply_with_marker(
                        &mut server, &auth,
                        json!({"access_token":FIXTURE_ACCESS,"scope":"mainaccount trade:read_write"}),
                        marker,
                    ).await;
                    for supplied_marker in [marker, !marker] {
                        let request = next_request(&mut server).await;
                        assert_eq!(request["method"], "private/move_positions");
                        assert_eq!(request["params"], json!({
                            "source_uid":101, "target_uid":102, "trades":[{"instrument_name":"BTC-PERPETUAL","amount":10}],
                            "access_token":FIXTURE_ACCESS,
                        }));
                        reply_with_marker(&mut server, &request, json!({"trades":[]}), supplied_marker).await;
                    }
                    assert!(timeout_at(Instant::now() + Duration::from_millis(15), server.next()).await.is_err());
                });
                let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
                let mut session = Session::new(
                    client, profile, Duration::from_secs(2), Some(fixture_credentials(profile)),
                );
                let mut output = Vec::new();
                let auth = session.authenticate(Map::new(), &mut output).await.unwrap();
                assert_eq!(auth["result"]["access_token"], "[REDACTED]");
                assert_eq!(session.granted_scope(), Some("mainaccount trade:read_write"));
                let params: Map<String,Value> = serde_json::from_value(json!({
                    "source_uid":101, "target_uid":102, "trades":[{"instrument_name":"BTC-PERPETUAL","amount":10}],
                })).unwrap();
                let reply = session.call("private/move_positions", params.clone(), true, &mut output).await.unwrap();
                assert_eq!(reply["result"], json!({"trades":[]}));
                assert_eq!(reply["testnet"], marker);
                let mismatch = session.call("private/move_positions", params, true, &mut output).await
                    .map_err(|error| uncertain_write(Effect::Special, error));
                assert!(matches!(mismatch, Err(Failure::UncertainWrite)));
                assert!(output.is_empty());
                server.await.unwrap();
            }
        });
    }

    fn fixture_credentials(profile: Profile) -> Credentials {
        Credentials {
            profile,
            id: "synthetic-client-id".to_owned(),
            secret: "synthetic-client-secret".to_owned(),
        }
    }

    async fn next_request<S: AsyncRead + AsyncWrite + Unpin>(
        server: &mut WebSocketStream<S>,
    ) -> Value {
        let frame = server.next().await.unwrap().unwrap().into_text().unwrap();
        serde_json::from_str(&frame).unwrap()
    }

    async fn reply<S: AsyncRead + AsyncWrite + Unpin>(
        server: &mut WebSocketStream<S>,
        request: &Value,
        result: Value,
    ) {
        reply_with_marker(server, request, result, true).await;
    }

    async fn reply_with_marker<S: AsyncRead + AsyncWrite + Unpin>(
        server: &mut WebSocketStream<S>,
        request: &Value,
        result: Value,
        marker: bool,
    ) {
        server
            .send(Message::Text(
                json!({
                    "jsonrpc": "2.0", "id": request["id"], "testnet": marker, "result": result,
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
    }
}
