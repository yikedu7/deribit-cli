//! Finite-deadline and response-budget primitives.

use std::fmt;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::configuration::{OperationTimeout, ResponseBodyLimit};
use crate::errors::CoreError;

/// A UUID v4 generated once for a future JSON-RPC operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestId(Uuid);

impl RequestId {
    /// Generates a fresh UUID v4.
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }

    /// Returns the UUID used in the JSON-RPC envelope.
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A deadline calculated once for an operation and shared by later stages.
#[derive(Clone, Copy, Debug)]
pub struct Deadline {
    expires_at: Instant,
    timeout: Duration,
}

impl Deadline {
    /// Calculates a deadline from a validated operation timeout.
    pub fn from_now(timeout: OperationTimeout) -> Self {
        Self {
            expires_at: Instant::now() + timeout.duration(),
            timeout: timeout.duration(),
        }
    }

    /// Returns the remaining duration or a stable timeout classification.
    pub fn remaining(self) -> Result<Duration, CoreError> {
        self.remaining_at(Instant::now())
    }

    fn remaining_at(self, now: Instant) -> Result<Duration, CoreError> {
        match self.expires_at.checked_duration_since(now) {
            Some(remaining) if !remaining.is_zero() => Ok(remaining),
            _ => Err(CoreError::Timeout {
                timeout: self.timeout,
            }),
        }
    }

    /// Returns whether the deadline has expired.
    pub fn is_expired(self) -> bool {
        self.remaining().is_err()
    }
}

/// A non-overflowing response byte budget for streaming reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseByteBudget {
    limit_bytes: usize,
    consumed_bytes: usize,
}

impl ResponseByteBudget {
    /// Starts with an empty budget constrained by a validated body limit.
    pub const fn new(limit: ResponseBodyLimit) -> Self {
        Self {
            limit_bytes: limit.bytes(),
            consumed_bytes: 0,
        }
    }

    /// Records a chunk only if it remains within the configured limit.
    pub fn consume(&mut self, byte_count: usize) -> Result<(), CoreError> {
        let Some(next) = self.consumed_bytes.checked_add(byte_count) else {
            return Err(CoreError::ResponseSize {
                limit_bytes: self.limit_bytes,
            });
        };
        if next > self.limit_bytes {
            return Err(CoreError::ResponseSize {
                limit_bytes: self.limit_bytes,
            });
        }

        self.consumed_bytes = next;
        Ok(())
    }

    /// Returns bytes already accepted into the native response.
    pub const fn consumed_bytes(self) -> usize {
        self.consumed_bytes
    }

    /// Returns bytes that may still be accepted.
    pub const fn remaining_bytes(self) -> usize {
        self.limit_bytes - self.consumed_bytes
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Deadline, RequestId, ResponseByteBudget};
    use crate::configuration::{OperationTimeout, ResponseBodyLimit};
    use crate::errors::{CoreError, ErrorClass};

    #[test]
    fn request_ids_are_v4_and_not_reused() {
        let first = RequestId::new_v4();
        let second = RequestId::new_v4();

        assert_eq!(first.as_uuid().get_version_num(), 4);
        assert_ne!(first, second);
    }

    #[test]
    fn response_budget_never_advances_past_its_limit() {
        let mut budget = ResponseByteBudget::new(ResponseBodyLimit::new(5).unwrap());
        budget.consume(3).unwrap();
        assert_eq!(budget.remaining_bytes(), 2);

        let error = budget.consume(3).unwrap_err();
        assert_eq!(error.class(), ErrorClass::Transport);
        assert!(matches!(error, CoreError::ResponseSize { limit_bytes: 5 }));
        assert_eq!(budget.consumed_bytes(), 3);
    }

    #[test]
    fn zero_remaining_deadline_has_timeout_classification() {
        let timeout = OperationTimeout::new(Duration::from_millis(1)).unwrap();
        let now = std::time::Instant::now();
        let deadline = Deadline {
            expires_at: now,
            timeout: timeout.duration(),
        };

        let error = deadline.remaining_at(now).unwrap_err();
        assert_eq!(error.class(), ErrorClass::Timeout);
    }
}
