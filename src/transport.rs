//! HTTP-independent request and response boundary for one frozen operation.
//!
//! This port intentionally does not expose `reqwest` types.  The only request
//! constructor is crate-visible and is used by the JSON-RPC adapter after a
//! [`crate::manifest::ValidatedOperation`] has been checked against the
//! embedded registry.

use std::time::Duration;

use thiserror::Error;
use url::Url;

use crate::errors::ErrorClass;
use crate::runtime::{Deadline, RequestId};

/// The sole HTTP method permitted for the frozen JSON-RPC transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpMethod {
    /// JSON-RPC 2.0 requests are always sent with HTTP POST.
    Post,
}

impl HttpMethod {
    /// Returns the wire spelling used by transport contract tests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Post => "POST",
        }
    }
}

/// A checked, one-shot request that contains no authentication material.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportRequest {
    endpoint: Url,
    method: HttpMethod,
    body: Vec<u8>,
    request_id: RequestId,
}

impl TransportRequest {
    /// Constructs a request only after the RPC adapter has validated it.
    ///
    /// The unforgeable permit is intentionally defined by the RPC module, so
    /// other crate modules cannot construct a URL/body pair that bypasses a
    /// manifest-validated operation.
    pub(crate) fn new(
        endpoint: Url,
        method: HttpMethod,
        body: Vec<u8>,
        request_id: RequestId,
        _permit: crate::rpc::RpcTransportPermit,
    ) -> Self {
        Self {
            endpoint,
            method,
            body,
            request_id,
        }
    }

    /// Returns the fixed HTTPS endpoint selected from the frozen environment.
    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    /// Returns the fixed HTTP method.
    pub const fn method(&self) -> HttpMethod {
        self.method
    }

    /// Returns the serialized JSON-RPC request body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Returns the only application-controlled request header value.
    ///
    /// The transport never accepts caller-supplied headers, so this fixed
    /// view lets an offline mock verify the JSON-RPC media type without
    /// creating an authentication or arbitrary-header escape hatch.
    pub const fn content_type(&self) -> &'static str {
        "application/json"
    }

    /// Returns the UUID associated with exactly this operation.
    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    /// Confirms the target remains in the only allowed HTTPS API namespace.
    pub(crate) fn has_frozen_target(&self) -> bool {
        self.endpoint.scheme() == "https"
            && self.endpoint.username().is_empty()
            && self.endpoint.password().is_none()
            && self.endpoint.query().is_none()
            && self.endpoint.fragment().is_none()
            && self.endpoint.path() == "/api/v2"
            && self.method == HttpMethod::Post
    }
}

/// The complete, bounded raw HTTP response returned by a transport adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportResponse {
    status: u16,
    body: Vec<u8>,
}

impl TransportResponse {
    /// Creates a raw response for an offline mock or a concrete adapter.
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self { status, body }
    }

    /// Returns the received HTTP status without interpreting it as an API result.
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Returns the raw response bytes before JSON-RPC validation.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Returns whether the response has a successful HTTP status class.
    pub const fn is_success_status(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}

/// A synchronous, single-request transport port.
///
/// Implementations must execute at most one HTTP request for each invocation.
/// They receive a prebuilt finite deadline and must not retry, redirect, batch,
/// paginate, or issue auxiliary requests.
pub trait Transport {
    /// Sends one checked request and returns the bounded raw response.
    fn execute(
        &self,
        request: &TransportRequest,
        deadline: Deadline,
    ) -> Result<TransportResponse, TransportError>;
}

/// Transport-local failures that preserve the frozen internal classification.
#[derive(Debug, Error)]
pub enum TransportError {
    /// The one-operation deadline elapsed before a complete response arrived.
    #[error("operation timed out")]
    Timeout,

    /// The frozen request policy rejected an internal invariant violation.
    #[error("transport policy rejected the request: {0}")]
    Policy(String),

    /// The blocking client could not establish or complete the request.
    #[error("HTTP request could not be completed")]
    Request,

    /// Reading the response body failed before a complete body was available.
    #[error("HTTP response body could not be read")]
    ResponseRead,

    /// The response exceeded the configured byte budget.
    #[error("HTTP response body exceeds the configured {limit_bytes}-byte budget")]
    ResponseSize { limit_bytes: usize },

    /// A frozen configuration could not be translated into the concrete client.
    #[error("blocking client could not be configured")]
    ClientConfiguration,

    /// An invariant outside user-controlled input failed.
    #[error("internal transport invariant failed: {0}")]
    Internal(String),
}

impl TransportError {
    /// Returns the frozen internal error class without choosing a process exit code.
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Timeout => ErrorClass::Timeout,
            Self::Policy(_) | Self::Internal(_) => ErrorClass::Internal,
            Self::Request
            | Self::ResponseRead
            | Self::ResponseSize { .. }
            | Self::ClientConfiguration => ErrorClass::Transport,
        }
    }

    /// Produces a timeout result when a shared deadline is exhausted.
    pub(crate) fn deadline_expired(_timeout: Duration) -> Self {
        Self::Timeout
    }
}

#[cfg(test)]
mod tests {
    use super::{HttpMethod, TransportResponse};

    #[test]
    fn response_keeps_status_and_bytes_uninterpreted() {
        let response = TransportResponse::new(400, br#"{\"error\":{\"code\":1}}"#.to_vec());

        assert_eq!(response.status(), 400);
        assert!(!response.is_success_status());
        assert_eq!(response.body(), br#"{\"error\":{\"code\":1}}"#);
        assert_eq!(HttpMethod::Post.as_str(), "POST");
    }
}
