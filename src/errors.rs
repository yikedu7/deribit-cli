//! Stable internal error classifications shared by later stages.
//!
//! This module intentionally does not decide process exit codes. The frozen
//! command-syntax contract owns that final mapping; consumers can use
//! ErrorClass to preserve the distinction across module boundaries.

use std::time::Duration;

use thiserror::Error;

/// Error classes that later adapters map to the frozen process boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorClass {
    /// Command spelling, policy, or configuration validation failed.
    Usage,
    /// User-provided content could not be parsed or validated.
    Input,
    /// HTTP, TLS, proxy, or response-size handling failed.
    Transport,
    /// An explicit local file or stream operation failed.
    LocalIo,
    /// A validated upstream JSON-RPC error remains API-native.
    ApiNative,
    /// An operation exceeded its finite deadline.
    Timeout,
    /// An invariant or frozen-artifact check failed.
    Internal,
}

impl ErrorClass {
    /// A stable lower-case label for diagnostics and tests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Input => "input",
            Self::Transport => "transport",
            Self::LocalIo => "local_io",
            Self::ApiNative => "api_native",
            Self::Timeout => "timeout",
            Self::Internal => "internal",
        }
    }
}

/// Errors produced by the S2 pure-core boundary.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("embedded manifest could not be parsed: {0}")]
    EmbeddedManifest(String),

    #[error("manifest validation failed: {0}")]
    ManifestValidation(String),

    #[error("unknown operation_id: {0}")]
    UnknownOperation(String),

    #[error("unknown canonical command: {0}")]
    UnknownCommand(String),

    #[error("command correspondence is invalid: {0}")]
    CommandCorrespondence(String),

    #[error("configuration policy rejected: {0}")]
    Configuration(String),

    #[error("proxy policy rejected: {0}")]
    Proxy(String),

    #[error("timeout policy rejected: {0}")]
    TimeoutPolicy(String),

    #[error("response body exceeds the configured {limit_bytes}-byte budget")]
    ResponseSize { limit_bytes: usize },

    #[error("operation timed out after {timeout:?}")]
    Timeout { timeout: Duration },

    #[error("input validation failed: {0}")]
    Input(String),

    #[error("internal invariant failed: {0}")]
    Internal(String),
}

impl CoreError {
    /// Returns the internal classification without imposing a process exit code.
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::UnknownOperation(_)
            | Self::UnknownCommand(_)
            | Self::CommandCorrespondence(_)
            | Self::Configuration(_)
            | Self::Proxy(_)
            | Self::TimeoutPolicy(_) => ErrorClass::Usage,
            Self::Input(_) => ErrorClass::Input,
            Self::ResponseSize { .. } => ErrorClass::Transport,
            Self::Timeout { .. } => ErrorClass::Timeout,
            Self::EmbeddedManifest(_) | Self::ManifestValidation(_) | Self::Internal(_) => {
                ErrorClass::Internal
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CoreError, ErrorClass};

    #[test]
    fn classifications_keep_policy_and_internal_errors_distinct() {
        assert_eq!(
            CoreError::Proxy("userinfo".into()).class(),
            ErrorClass::Usage
        );
        assert_eq!(
            CoreError::ManifestValidation("drift".into()).class(),
            ErrorClass::Internal
        );
    }
}
