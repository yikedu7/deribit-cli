//! JSON-RPC 2.0 framing for one manifest-validated public operation.
//!
//! This module owns the small typed envelope around an otherwise opaque
//! API-native JSON response.  It never manufactures a method string, endpoint,
//! batch, notification, retry, or pagination request.

use serde_json::{Map, Value};
use thiserror::Error;

use crate::configuration::ClientConfiguration;
use crate::errors::{CoreError, ErrorClass};
use crate::manifest::ValidatedOperation;
use crate::runtime::RequestId;
use crate::transport::{HttpMethod, TransportRequest, TransportResponse};

/// A crate-private capability that only this module can construct.
///
/// `TransportRequest` requires this value so every request URL and JSON-RPC
/// method is derived together from a [`ValidatedOperation`].
pub(crate) struct RpcTransportPermit(());

impl RpcTransportPermit {
    fn new() -> Self {
        Self(())
    }
}

/// A checked JSON-RPC request derived from an immutable manifest operation.
#[derive(Clone, Debug)]
pub struct RpcRequest<'a> {
    operation: ValidatedOperation<'a>,
    request_id: RequestId,
    params: Map<String, Value>,
}

impl<'a> RpcRequest<'a> {
    /// Creates a one-shot named-parameters request for a validated operation.
    pub fn new(
        operation: ValidatedOperation<'a>,
        request_id: RequestId,
        params: Map<String, Value>,
    ) -> Result<Self, RpcError> {
        validate_operation_contract(operation)?;
        Ok(Self {
            operation,
            request_id,
            params,
        })
    }

    /// Returns the registry-backed operation identifier.
    pub fn operation_id(&self) -> &str {
        self.operation.operation_id()
    }

    /// Returns the upstream public JSON-RPC method identifier.
    pub fn method(&self) -> &str {
        self.operation.method()
    }

    /// Returns the UUID generated once for this operation.
    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    /// Returns the named parameters without adding local defaults or metadata.
    pub fn params(&self) -> &Map<String, Value> {
        &self.params
    }

    /// Builds the fixed POST request from the selected frozen environment.
    pub fn to_transport_request(
        &self,
        configuration: &ClientConfiguration,
    ) -> Result<TransportRequest, RpcError> {
        let mut endpoint = configuration.endpoint().map_err(RpcError::Configuration)?;
        if endpoint.scheme() != "https" {
            return Err(RpcError::Protocol(
                "the frozen environment did not resolve to HTTPS".into(),
            ));
        }

        endpoint.set_path(self.operation.request_path());
        endpoint.set_query(None);
        endpoint.set_fragment(None);

        let body = serde_json::to_vec(&self.envelope())
            .map_err(|error| RpcError::Internal(error.to_string()))?;
        let request = TransportRequest::new(
            endpoint,
            HttpMethod::Post,
            body,
            self.request_id,
            RpcTransportPermit::new(),
        );
        if !request.has_frozen_target() {
            return Err(RpcError::Protocol(
                "manifest operation resolved outside the frozen public HTTPS namespace".into(),
            ));
        }
        Ok(request)
    }

    /// Parses a bounded response while preserving the complete native JSON value.
    pub fn parse_response(&self, response: TransportResponse) -> Result<RpcResponse, RpcError> {
        let native = serde_json::from_slice::<Value>(response.body())
            .map_err(|_| RpcError::InvalidJsonResponse)?;
        let object = native.as_object().ok_or(RpcError::InvalidJsonResponse)?;

        if !response.is_success_status() {
            if is_valid_api_error_envelope(object, self.request_id) {
                return Ok(RpcResponse::ApiError { native });
            }
            return Err(RpcError::HttpStatusWithoutApiError {
                status: response.status(),
            });
        }

        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(RpcError::Protocol(
                "response jsonrpc version must be exactly 2.0".into(),
            ));
        }
        if object.get("id") != Some(&Value::String(self.request_id.to_string())) {
            return Err(RpcError::Protocol(
                "response id does not match the one operation request id".into(),
            ));
        }

        let has_result = object.contains_key("result");
        let has_error = object.contains_key("error");
        if has_result == has_error {
            return Err(RpcError::Protocol(
                "response must contain exactly one of result or error".into(),
            ));
        }

        if has_error {
            if !is_api_error_object(object.get("error")) {
                return Err(RpcError::Protocol(
                    "response error must be a valid JSON-RPC error object".into(),
                ));
            }
            return Ok(RpcResponse::ApiError { native });
        }

        Ok(RpcResponse::Success { native })
    }

    fn envelope(&self) -> Value {
        let mut envelope = Map::new();
        envelope.insert("jsonrpc".into(), Value::String("2.0".into()));
        envelope.insert("id".into(), Value::String(self.request_id.to_string()));
        envelope.insert(
            "method".into(),
            Value::String(self.operation.method().into()),
        );
        envelope.insert("params".into(), Value::Object(self.params.clone()));
        Value::Object(envelope)
    }
}

/// The API-native JSON-RPC result, kept separate from local diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub enum RpcResponse {
    /// A valid success envelope with its native JSON preserved unchanged.
    Success { native: Value },
    /// A valid API-native error envelope with its native JSON preserved unchanged.
    ApiError { native: Value },
}

impl RpcResponse {
    /// Returns the entire unmodified JSON-RPC response envelope.
    pub fn native(&self) -> &Value {
        match self {
            Self::Success { native } | Self::ApiError { native } => native,
        }
    }

    /// Returns whether the response is an API-native error rather than success.
    pub const fn is_api_error(&self) -> bool {
        matches!(self, Self::ApiError { .. })
    }

    /// Consumes the wrapper and returns the exact native JSON value.
    pub fn into_native(self) -> Value {
        match self {
            Self::Success { native } | Self::ApiError { native } => native,
        }
    }
}

/// JSON-RPC framing failures, deliberately separate from API-native errors.
#[derive(Debug, Error)]
pub enum RpcError {
    /// A frozen core configuration could not produce its fixed endpoint.
    #[error("frozen configuration could not resolve an endpoint: {0}")]
    Configuration(#[source] CoreError),

    /// The HTTP response did not contain valid JSON.
    #[error("HTTP response is not valid JSON")]
    InvalidJsonResponse,

    /// A non-success HTTP status was not a valid API-native error envelope.
    #[error("HTTP status {status} did not carry an API-native error")]
    HttpStatusWithoutApiError { status: u16 },

    /// A JSON-RPC protocol invariant failed.
    #[error("JSON-RPC protocol invariant failed: {0}")]
    Protocol(String),

    /// Serialization of the project-owned envelope unexpectedly failed.
    #[error("JSON-RPC envelope could not be serialized: {0}")]
    Internal(String),
}

impl RpcError {
    /// Returns the frozen internal error classification without a process code.
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Configuration(error) => error.class(),
            Self::InvalidJsonResponse | Self::HttpStatusWithoutApiError { .. } => {
                ErrorClass::Transport
            }
            Self::Protocol(_) | Self::Internal(_) => ErrorClass::Internal,
        }
    }
}

fn is_api_error_object(error: Option<&Value>) -> bool {
    let Some(error) = error.and_then(Value::as_object) else {
        return false;
    };

    error.get("code").is_some_and(Value::is_number)
        && error.get("message").is_some_and(Value::is_string)
}

fn is_valid_api_error_envelope(object: &Map<String, Value>, request_id: RequestId) -> bool {
    object.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && object.get("id") == Some(&Value::String(request_id.to_string()))
        && !object.contains_key("result")
        && is_api_error_object(object.get("error"))
}

fn validate_operation_contract(operation: ValidatedOperation<'_>) -> Result<(), RpcError> {
    if !operation.is_public_read_only()
        || !operation.method().starts_with("public/")
        || operation.implementation_http_method() != "POST"
        || operation.jsonrpc_version() != "2.0"
        || operation.request_shape() != "named_params"
        || operation.request_path() != "/api/v2"
    {
        return Err(RpcError::Protocol(
            "operation does not satisfy the frozen public JSON-RPC policy".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::{RpcError, RpcRequest};
    use crate::configuration::ClientConfiguration;
    use crate::manifest::ManifestRegistry;
    use crate::runtime::RequestId;
    use crate::transport::{HttpMethod, TransportResponse};

    fn order_book_request() -> RpcRequest<'static> {
        let registry = Box::leak(Box::new(ManifestRegistry::embedded().unwrap()));
        let operation = registry.operation("public_get_order_book").unwrap();
        let mut params = Map::new();
        params.insert(
            "instrument_name".into(),
            Value::String("BTC-PERPETUAL".into()),
        );
        params.insert("depth".into(), json!(5));
        RpcRequest::new(operation, RequestId::new_v4(), params).unwrap()
    }

    #[test]
    fn registry_backed_request_has_only_the_frozen_envelope_and_endpoint() {
        let request = order_book_request();
        let transport = request
            .to_transport_request(&ClientConfiguration::default())
            .unwrap();

        assert_eq!(transport.method(), HttpMethod::Post);
        assert_eq!(
            transport.endpoint().as_str(),
            "https://www.deribit.com/api/v2"
        );
        let body: Value = serde_json::from_slice(transport.body()).unwrap();
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["method"], "public/get_order_book");
        assert_eq!(body["params"]["instrument_name"], "BTC-PERPETUAL");
        assert_eq!(body["params"]["depth"], 5);
        assert_eq!(body.as_object().unwrap().len(), 4);
    }

    #[test]
    fn api_error_is_preserved_even_when_http_status_is_an_error() {
        let request = order_book_request();
        let response = json!({
            "jsonrpc": "2.0",
            "id": request.request_id().to_string(),
            "error": {"code": 10004, "message": "not_found", "data": {"future": null}}
        });
        let parsed = request
            .parse_response(TransportResponse::new(
                400,
                serde_json::to_vec(&response).unwrap(),
            ))
            .unwrap();

        assert!(parsed.is_api_error());
        assert_eq!(parsed.native(), &response);
    }

    #[test]
    fn non_api_http_failure_and_mismatched_id_do_not_become_native_results() {
        let request = order_book_request();
        let response = json!({
            "jsonrpc": "2.0",
            "id": request.request_id().to_string(),
            "result": {"ignored": true}
        });
        assert!(matches!(
            request.parse_response(TransportResponse::new(
                502,
                serde_json::to_vec(&response).unwrap()
            )),
            Err(RpcError::HttpStatusWithoutApiError { status: 502 })
        ));

        let wrong_id = json!({"jsonrpc": "2.0", "id": "not-this-request", "result": null});
        assert!(matches!(
            request.parse_response(TransportResponse::new(
                200,
                serde_json::to_vec(&wrong_id).unwrap()
            )),
            Err(RpcError::Protocol(_))
        ));
    }

    #[test]
    fn malformed_http_error_object_is_transport_not_api_native() {
        let request = order_book_request();
        let malformed_error = json!({
            "jsonrpc": "2.0",
            "id": request.request_id().to_string(),
            "error": {"message": "missing numeric code"}
        });

        assert!(matches!(
            request.parse_response(TransportResponse::new(
                502,
                serde_json::to_vec(&malformed_error).unwrap()
            )),
            Err(RpcError::HttpStatusWithoutApiError { status: 502 })
        ));
    }
}
