//! Blocking `reqwest` implementation of the frozen one-shot transport port.
//!
//! The implementation configures rustls, a finite deadline, no automatic
//! environment proxy discovery, explicit allowlisted proxies, no retry, and no
//! redirect following.  It does not expose an async API or a `reqwest` type
//! through [`crate::transport::Transport`].

use std::cmp;
use std::io::{ErrorKind, Read};

use reqwest::blocking::{Client, Response};
use reqwest::{NoProxy, Proxy, redirect, retry};

use crate::configuration::{ClientConfiguration, ResponseBodyLimit};
use crate::runtime::{Deadline, ResponseByteBudget};
use crate::transport::{Transport, TransportError, TransportRequest, TransportResponse};

/// The concrete blocking transport for one immutable client configuration.
#[derive(Clone, Debug)]
pub struct ReqwestTransport {
    configuration: ClientConfiguration,
}

impl ReqwestTransport {
    /// Binds the adapter to a configuration that has no user-supplied endpoint.
    pub fn new(configuration: ClientConfiguration) -> Self {
        Self { configuration }
    }

    /// Returns the immutable configuration consumed by this adapter.
    pub fn configuration(&self) -> &ClientConfiguration {
        &self.configuration
    }

    fn client_for_deadline(&self, deadline: Deadline) -> Result<Client, TransportError> {
        let remaining = deadline.remaining().map_err(|error| {
            TransportError::deadline_expired(match error {
                crate::errors::CoreError::Timeout { timeout } => timeout,
                _ => self.configuration.operation_timeout().duration(),
            })
        })?;
        let connect_timeout = cmp::min(self.configuration.connect_timeout().duration(), remaining);

        let mut builder = Client::builder()
            .use_rustls_tls()
            .redirect(redirect::Policy::none())
            .retry(retry::never())
            .referer(false)
            .no_proxy()
            .timeout(remaining)
            .connect_timeout(connect_timeout);

        let no_proxy = self
            .configuration
            .proxies()
            .no_proxy()
            .and_then(NoProxy::from_string);
        if let Some(url) = self.configuration.proxies().https_proxy() {
            let proxy =
                Proxy::https(url.as_str()).map_err(|_| TransportError::ClientConfiguration)?;
            builder = builder.proxy(proxy_with_exclusions(proxy, &no_proxy));
        }
        if let Some(url) = self.configuration.proxies().http_proxy() {
            let proxy =
                Proxy::http(url.as_str()).map_err(|_| TransportError::ClientConfiguration)?;
            builder = builder.proxy(proxy_with_exclusions(proxy, &no_proxy));
        }
        if let Some(url) = self.configuration.proxies().all_proxy() {
            let proxy =
                Proxy::all(url.as_str()).map_err(|_| TransportError::ClientConfiguration)?;
            builder = builder.proxy(proxy_with_exclusions(proxy, &no_proxy));
        }

        builder
            .build()
            .map_err(|_| TransportError::ClientConfiguration)
    }

    fn has_configured_target(&self, request: &TransportRequest) -> Result<bool, TransportError> {
        let configured_endpoint = self
            .configuration
            .endpoint()
            .map_err(|_| TransportError::ClientConfiguration)?;
        let endpoint = request.endpoint();

        Ok(request.has_frozen_target()
            && endpoint.scheme() == configured_endpoint.scheme()
            && endpoint.host_str() == configured_endpoint.host_str()
            && endpoint.port_or_known_default() == configured_endpoint.port_or_known_default()
            && endpoint.path() == configured_endpoint.path())
    }
}

impl Transport for ReqwestTransport {
    fn execute(
        &self,
        request: &TransportRequest,
        deadline: Deadline,
    ) -> Result<TransportResponse, TransportError> {
        if !self.has_configured_target(request)? {
            return Err(TransportError::Policy(
                "request did not retain the configured frozen HTTPS public target".into(),
            ));
        }

        let client = self.client_for_deadline(deadline)?;
        let response = client
            .post(request.endpoint().clone())
            .header("content-type", request.content_type())
            .body(request.body().to_vec())
            .send()
            .map_err(|error| request_error(error, deadline))?;
        let status = response.status().as_u16();
        let body = read_bounded_body(response, self.configuration.response_body_limit(), deadline)?;

        Ok(TransportResponse::new(status, body))
    }
}

fn proxy_with_exclusions(proxy: Proxy, no_proxy: &Option<NoProxy>) -> Proxy {
    proxy.no_proxy(no_proxy.clone())
}

fn request_error(error: reqwest::Error, deadline: Deadline) -> TransportError {
    if deadline.is_expired() || error.is_timeout() {
        TransportError::Timeout
    } else {
        TransportError::Request
    }
}

fn response_read_error(error: std::io::Error, deadline: Deadline) -> TransportError {
    let reqwest_timeout = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<reqwest::Error>())
        .is_some_and(reqwest::Error::is_timeout);

    if deadline.is_expired() || error.kind() == ErrorKind::TimedOut || reqwest_timeout {
        TransportError::Timeout
    } else {
        TransportError::ResponseRead
    }
}

fn read_bounded_body(
    mut response: Response,
    limit: ResponseBodyLimit,
    deadline: Deadline,
) -> Result<Vec<u8>, TransportError> {
    let limit_bytes = limit.bytes();
    let mut budget = ResponseByteBudget::new(limit);
    let mut body = Vec::with_capacity(cmp::min(limit_bytes, 8 * 1024));
    let mut chunk = [0_u8; 8 * 1024];

    loop {
        if deadline.is_expired() {
            return Err(TransportError::Timeout);
        }
        let count = response
            .read(&mut chunk)
            .map_err(|error| response_read_error(error, deadline))?;
        if count == 0 {
            break;
        }
        budget
            .consume(count)
            .map_err(|_| TransportError::ResponseSize { limit_bytes })?;
        body.extend_from_slice(&chunk[..count]);
    }

    if deadline.is_expired() {
        return Err(TransportError::Timeout);
    }
    Ok(body)
}

// The loopback harness is deliberately compiled only into this source unit's
// tests.  It is not a product endpoint override or a public transport API.
#[cfg(test)]
#[path = "../tests/common/loopback.rs"]
mod test_loopback;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Map, Value, json};
    use url::Url;

    use super::{ReqwestTransport, read_bounded_body, request_error, response_read_error};
    use crate::configuration::{
        ClientConfiguration, ConnectTimeout, Environment, OperationTimeout, ProxyEnvironment,
        ProxySettings, ResponseBodyLimit,
    };
    use crate::manifest::ManifestRegistry;
    use crate::rpc::RpcRequest;
    use crate::runtime::{Deadline, RequestId};
    use crate::transport::{TransportError, TransportRequest};

    use super::test_loopback::{LoopbackServer, ResponsePlan};

    impl ReqwestTransport {
        /// Sends a frozen request to a numeric loopback listener for unit tests.
        ///
        /// This intentionally-private test seam preserves the generated public
        /// request path while replacing only the network route.  Normal builds
        /// cannot compile or call it.
        fn execute_against_loopback_for_test(
            &self,
            request: &TransportRequest,
            deadline: Deadline,
            loopback_target: &Url,
        ) -> Result<crate::transport::TransportResponse, TransportError> {
            if !is_loopback_test_target(request, loopback_target) {
                return Err(TransportError::Policy(
                    "test seam accepts only a path-preserving numeric loopback HTTP target".into(),
                ));
            }

            let client = self.client_for_deadline(deadline)?;
            let response = client
                .post(loopback_target.clone())
                .header("content-type", request.content_type())
                .body(request.body().to_vec())
                .send()
                .map_err(|error| request_error(error, deadline))?;
            let status = response.status().as_u16();
            let body =
                read_bounded_body(response, self.configuration.response_body_limit(), deadline)?;

            Ok(crate::transport::TransportResponse::new(status, body))
        }
    }

    fn is_loopback_test_target(request: &TransportRequest, target: &Url) -> bool {
        target.scheme() == "http"
            && target.host_str() == Some("127.0.0.1")
            && target.port().is_some()
            && target.username().is_empty()
            && target.password().is_none()
            && target.query().is_none()
            && target.fragment().is_none()
            && target.path() == request.endpoint().path()
    }

    fn order_book_transport_request() -> TransportRequest {
        let registry = ManifestRegistry::embedded().unwrap();
        let operation = registry.operation("public_get_order_book").unwrap();
        let mut params = Map::new();
        params.insert(
            "instrument_name".into(),
            Value::String("BTC-PERPETUAL".into()),
        );
        params.insert("depth".into(), json!(5));

        RpcRequest::new(operation, RequestId::new_v4(), params)
            .unwrap()
            .to_transport_request(&ClientConfiguration::default())
            .unwrap()
    }

    fn test_configuration(
        operation_timeout: Duration,
        response_body_limit: usize,
    ) -> ClientConfiguration {
        let connect_timeout = ConnectTimeout::new(operation_timeout).unwrap();
        ClientConfiguration::new(
            Environment::Mainnet,
            OperationTimeout::new(operation_timeout).unwrap(),
            connect_timeout,
            ResponseBodyLimit::new(response_body_limit).unwrap(),
            ProxySettings::default(),
        )
        .unwrap()
    }

    fn assert_one_complete_request(server: &LoopbackServer) {
        assert!(server.wait_for_request_count(1, Duration::from_secs(1)));
        assert_eq!(server.request_count(), 1);
        assert!(server.failure().is_none());
    }

    #[test]
    fn construction_keeps_the_only_configured_environment_and_proxy_policy() {
        let configuration = ClientConfiguration::new(
            Environment::Testnet,
            Default::default(),
            Default::default(),
            Default::default(),
            ProxySettings::from_environment(ProxyEnvironment {
                https_proxy: Some("https://proxy.invalid:8443".into()),
                ..ProxyEnvironment::default()
            })
            .unwrap(),
        )
        .unwrap();
        let transport = ReqwestTransport::new(configuration.clone());

        assert_eq!(transport.configuration(), &configuration);
        assert_eq!(
            transport.configuration().environment(),
            Environment::Testnet
        );
        assert!(transport.configuration().proxies().https_proxy().is_some());
    }

    #[test]
    fn non_timeout_response_read_errors_remain_transport_read_errors() {
        let deadline = Deadline::from_now(OperationTimeout::new(Duration::from_secs(1)).unwrap());
        let error = response_read_error(std::io::Error::other("loopback read failed"), deadline);

        assert!(matches!(error, TransportError::ResponseRead));
    }

    #[test]
    fn no_retry_no_redirect_no_implicit_pagination() {
        let request = order_book_transport_request();
        let transport = ReqwestTransport::new(test_configuration(Duration::from_secs(1), 1024));

        let redirect_server = LoopbackServer::start(
            ResponsePlan::new(302, Vec::new()).with_header("Location", "/api/v2/public/next"),
        )
        .unwrap();
        let redirect = transport
            .execute_against_loopback_for_test(
                &request,
                Deadline::from_now(transport.configuration().operation_timeout()),
                &redirect_server.url(request.endpoint().path()),
            )
            .unwrap();
        assert_eq!(redirect.status(), 302);
        assert_one_complete_request(&redirect_server);
        let captured = redirect_server.first_request().unwrap();
        assert_eq!(captured.method(), "POST");
        assert_eq!(captured.target(), request.endpoint().path());
        assert_eq!(captured.header("content-type"), Some("application/json"));
        assert_eq!(captured.body(), request.body());

        let retry_server = LoopbackServer::start(ResponsePlan::close_without_response()).unwrap();
        let retry = transport.execute_against_loopback_for_test(
            &request,
            Deadline::from_now(transport.configuration().operation_timeout()),
            &retry_server.url(request.endpoint().path()),
        );
        assert!(matches!(retry, Err(TransportError::Request)));
        assert_one_complete_request(&retry_server);

        let continuation = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": request.request_id().to_string(),
            "result": {"has_more": true, "continuation": "next-page"}
        }))
        .unwrap();
        let pagination_server =
            LoopbackServer::start(ResponsePlan::new(200, continuation)).unwrap();
        let response = transport
            .execute_against_loopback_for_test(
                &request,
                Deadline::from_now(transport.configuration().operation_timeout()),
                &pagination_server.url(request.endpoint().path()),
            )
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(
            response
                .body()
                .windows("continuation".len())
                .any(|window| window == b"continuation")
        );
        assert_one_complete_request(&pagination_server);
    }

    #[test]
    fn timeout_and_size_limits() {
        let request = order_book_transport_request();
        let timeout_transport =
            ReqwestTransport::new(test_configuration(Duration::from_millis(100), 1024));
        let slow_server = LoopbackServer::start(
            ResponsePlan::new(200, br#"{\"jsonrpc\":\"2.0\"}"#.to_vec())
                .with_head_delay(Duration::from_millis(400)),
        )
        .unwrap();
        let timed_out = timeout_transport.execute_against_loopback_for_test(
            &request,
            Deadline::from_now(timeout_transport.configuration().operation_timeout()),
            &slow_server.url(request.endpoint().path()),
        );
        assert!(
            matches!(timed_out, Err(TransportError::Timeout)),
            "slow-header request must exhaust the shared deadline, got {timed_out:?}"
        );
        assert_one_complete_request(&slow_server);

        let slow_body_server = LoopbackServer::start(
            ResponsePlan::new(200, br#"{\"jsonrpc\":\"2.0\"}"#.to_vec())
                .with_body_delay(Duration::from_millis(400)),
        )
        .unwrap();
        let slow_body = timeout_transport.execute_against_loopback_for_test(
            &request,
            Deadline::from_now(timeout_transport.configuration().operation_timeout()),
            &slow_body_server.url(request.endpoint().path()),
        );
        assert!(
            matches!(slow_body, Err(TransportError::Timeout)),
            "slow-body request must exhaust the shared deadline, got {slow_body:?}"
        );
        assert_one_complete_request(&slow_body_server);

        let size_transport = ReqwestTransport::new(test_configuration(Duration::from_secs(1), 8));
        let oversized_server =
            LoopbackServer::start(ResponsePlan::new(200, b"123456789".to_vec())).unwrap();
        let oversized = size_transport.execute_against_loopback_for_test(
            &request,
            Deadline::from_now(size_transport.configuration().operation_timeout()),
            &oversized_server.url(request.endpoint().path()),
        );
        match oversized {
            Err(TransportError::ResponseSize { limit_bytes: 8 }) => {}
            Ok(response) => panic!(
                "response budget must not return an incomplete body: {:?}",
                response.body()
            ),
            Err(other) => panic!("response budget had an unexpected error: {other:?}"),
        }
        assert_one_complete_request(&oversized_server);
    }
}
