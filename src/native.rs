//! Immutable API-native JSON response boundary.
//!
//! The value is never projected into a complete typed Deribit model.  This
//! preserves unknown fields, explicit nulls, array order, and arbitrary-
//! precision JSON number lexemes for the machine output contract.

use serde_json::Value;

use crate::rpc::RpcResponse;

/// Classification of one validated upstream JSON-RPC response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeKind {
    Success,
    ApiError,
}

/// An immutable, complete API-native JSON-RPC response envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeResponse {
    kind: NativeKind,
    value: Value,
}

impl NativeResponse {
    /// Converts the validated RPC boundary without altering its JSON value.
    pub(crate) fn from_rpc(response: RpcResponse) -> Self {
        let kind = if response.is_api_error() {
            NativeKind::ApiError
        } else {
            NativeKind::Success
        };
        Self {
            kind,
            value: response.into_native(),
        }
    }

    /// Returns the immutable response classification.
    pub const fn kind(&self) -> NativeKind {
        self.kind
    }

    /// Returns the complete native envelope by immutable reference.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Whether the upstream response is a validated API-native error.
    pub const fn is_api_error(&self) -> bool {
        matches!(self.kind, NativeKind::ApiError)
    }

    /// Consumes the wrapper without projecting or rewriting its JSON value.
    pub fn into_value(self) -> Value {
        self.value
    }
}
