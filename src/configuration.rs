//! Fixed endpoint, proxy, TLS, timeout, and size-limit policy.
//!
//! The configuration boundary does not create an HTTP client. It makes the
//! only permitted values explicit so that the S4 transport can consume a
//! validated policy without gaining an endpoint or credential escape hatch.

use std::env;
use std::time::Duration;

use url::Url;

use crate::errors::CoreError;

/// The only endpoints allowed by the frozen v1 contract.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Environment {
    #[default]
    Mainnet,
    Testnet,
}

impl Environment {
    /// Returns the frozen CLI spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
        }
    }

    /// Returns the corresponding fixed HTTPS JSON-RPC endpoint.
    pub fn endpoint(self) -> Result<Url, CoreError> {
        let raw = match self {
            Self::Mainnet => "https://www.deribit.com/api/v2",
            Self::Testnet => "https://test.deribit.com/api/v2",
        };

        Url::parse(raw).map_err(|error| {
            CoreError::Internal(format!("frozen endpoint {raw:?} is invalid: {error}"))
        })
    }
}

/// v1 permits only rustls-backed, certificate-validating TLS.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TlsPolicy {
    #[default]
    Rustls,
}

/// A finite operation deadline. The command layer owns textual parsing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationTimeout(Duration);

impl OperationTimeout {
    /// The frozen default for a complete operation.
    pub const DEFAULT: Self = Self(Duration::from_secs(10));
    /// The frozen upper limit for a complete operation.
    pub const MAXIMUM: Duration = Duration::from_secs(120);

    /// Validates a finite positive timeout within the v1 maximum.
    pub fn new(value: Duration) -> Result<Self, CoreError> {
        if value.is_zero() {
            return Err(CoreError::TimeoutPolicy(
                "operation timeout must be greater than zero".into(),
            ));
        }
        if value > Self::MAXIMUM {
            return Err(CoreError::TimeoutPolicy(
                "operation timeout must not exceed 120 seconds".into(),
            ));
        }
        Ok(Self(value))
    }

    /// Returns the validated duration.
    pub const fn duration(self) -> Duration {
        self.0
    }
}

impl Default for OperationTimeout {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A finite connection phase timeout, bounded by the complete operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectTimeout(Duration);

impl ConnectTimeout {
    /// The frozen default for connection establishment.
    pub const DEFAULT: Self = Self(Duration::from_secs(5));

    /// Validates a finite positive connection timeout.
    pub fn new(value: Duration) -> Result<Self, CoreError> {
        if value.is_zero() {
            return Err(CoreError::TimeoutPolicy(
                "connect timeout must be greater than zero".into(),
            ));
        }
        if value > OperationTimeout::MAXIMUM {
            return Err(CoreError::TimeoutPolicy(
                "connect timeout must not exceed 120 seconds".into(),
            ));
        }
        Ok(Self(value))
    }

    /// Returns the validated duration.
    pub const fn duration(self) -> Duration {
        self.0
    }
}

impl Default for ConnectTimeout {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The maximum accepted native response body size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseBodyLimit(usize);

impl ResponseBodyLimit {
    /// The frozen 16 MiB v1 body budget.
    pub const MAXIMUM_BYTES: usize = 16 * 1024 * 1024;
    /// The default is also the hard v1 upper bound.
    pub const DEFAULT: Self = Self(Self::MAXIMUM_BYTES);

    /// Validates a nonzero body budget at or below the frozen maximum.
    pub fn new(value: usize) -> Result<Self, CoreError> {
        if value == 0 {
            return Err(CoreError::Configuration(
                "response body limit must be greater than zero".into(),
            ));
        }
        if value > Self::MAXIMUM_BYTES {
            return Err(CoreError::Configuration(format!(
                "response body limit must not exceed {} bytes",
                Self::MAXIMUM_BYTES
            )));
        }
        Ok(Self(value))
    }

    /// Returns the validated byte limit.
    pub const fn bytes(self) -> usize {
        self.0
    }
}

impl Default for ResponseBodyLimit {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Normalized values from the only proxy environment variables v1 recognizes.
///
/// Credentials are intentionally absent from this type and cannot affect the
/// policy.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProxyEnvironment {
    pub https_proxy: Option<String>,
    pub http_proxy: Option<String>,
    pub all_proxy: Option<String>,
    pub no_proxy: Option<String>,
}

/// Validated proxy settings for a future blocking transport.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProxySettings {
    https_proxy: Option<Url>,
    http_proxy: Option<Url>,
    all_proxy: Option<Url>,
    no_proxy: Option<String>,
}

impl ProxySettings {
    /// Reads only the frozen proxy allowlist from the process environment.
    ///
    /// Uppercase has deterministic precedence over lowercase. This method never
    /// enumerates the environment and never reads credential, endpoint, method,
    /// or timeout variables.
    pub fn from_process_environment() -> Result<Self, CoreError> {
        Self::from_environment(ProxyEnvironment {
            https_proxy: allowed_environment_value("HTTPS_PROXY", "https_proxy")?,
            http_proxy: allowed_environment_value("HTTP_PROXY", "http_proxy")?,
            all_proxy: allowed_environment_value("ALL_PROXY", "all_proxy")?,
            no_proxy: allowed_environment_value("NO_PROXY", "no_proxy")?,
        })
    }

    /// Validates explicitly supplied allowlisted proxy values.
    pub fn from_environment(values: ProxyEnvironment) -> Result<Self, CoreError> {
        Ok(Self {
            https_proxy: parse_proxy("HTTPS_PROXY", values.https_proxy)?,
            http_proxy: parse_proxy("HTTP_PROXY", values.http_proxy)?,
            all_proxy: parse_proxy("ALL_PROXY", values.all_proxy)?,
            no_proxy: parse_no_proxy(values.no_proxy)?,
        })
    }

    /// Returns the explicit HTTPS proxy, if any.
    pub fn https_proxy(&self) -> Option<&Url> {
        self.https_proxy.as_ref()
    }

    /// Returns the explicit HTTP proxy, if any.
    pub fn http_proxy(&self) -> Option<&Url> {
        self.http_proxy.as_ref()
    }

    /// Returns the explicit fallback proxy, if any.
    pub fn all_proxy(&self) -> Option<&Url> {
        self.all_proxy.as_ref()
    }

    /// Returns the validated no-proxy list, if any.
    pub fn no_proxy(&self) -> Option<&str> {
        self.no_proxy.as_deref()
    }

    /// Whether any proxy routing setting is present.
    pub fn is_configured(&self) -> bool {
        self.https_proxy.is_some()
            || self.http_proxy.is_some()
            || self.all_proxy.is_some()
            || self.no_proxy.is_some()
    }
}

/// Fully validated core configuration for one future operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientConfiguration {
    environment: Environment,
    operation_timeout: OperationTimeout,
    connect_timeout: ConnectTimeout,
    response_body_limit: ResponseBodyLimit,
    tls_policy: TlsPolicy,
    proxies: ProxySettings,
}

impl ClientConfiguration {
    /// Constructs a policy without accepting a caller-provided endpoint.
    pub fn new(
        environment: Environment,
        operation_timeout: OperationTimeout,
        connect_timeout: ConnectTimeout,
        response_body_limit: ResponseBodyLimit,
        proxies: ProxySettings,
    ) -> Result<Self, CoreError> {
        if connect_timeout.duration() > operation_timeout.duration() {
            return Err(CoreError::TimeoutPolicy(
                "connect timeout must not exceed the operation timeout".into(),
            ));
        }

        Ok(Self {
            environment,
            operation_timeout,
            connect_timeout,
            response_body_limit,
            tls_policy: TlsPolicy::Rustls,
            proxies,
        })
    }

    /// Returns the selected fixed environment.
    pub const fn environment(&self) -> Environment {
        self.environment
    }

    /// Resolves the fixed endpoint for the selected environment.
    pub fn endpoint(&self) -> Result<Url, CoreError> {
        self.environment.endpoint()
    }

    /// Returns the complete operation timeout.
    pub const fn operation_timeout(&self) -> OperationTimeout {
        self.operation_timeout
    }

    /// Returns the connection phase timeout.
    pub const fn connect_timeout(&self) -> ConnectTimeout {
        self.connect_timeout
    }

    /// Returns the response body limit.
    pub const fn response_body_limit(&self) -> ResponseBodyLimit {
        self.response_body_limit
    }

    /// Returns the immutable TLS policy.
    pub const fn tls_policy(&self) -> TlsPolicy {
        self.tls_policy
    }

    /// Returns validated proxy routing settings.
    pub fn proxies(&self) -> &ProxySettings {
        &self.proxies
    }
}

impl Default for ClientConfiguration {
    fn default() -> Self {
        Self {
            environment: Environment::Mainnet,
            operation_timeout: OperationTimeout::DEFAULT,
            connect_timeout: ConnectTimeout::DEFAULT,
            response_body_limit: ResponseBodyLimit::DEFAULT,
            tls_policy: TlsPolicy::Rustls,
            proxies: ProxySettings::default(),
        }
    }
}

fn allowed_environment_value(upper: &str, lower: &str) -> Result<Option<String>, CoreError> {
    let uppercase = environment_string(upper)?;
    let lowercase = environment_string(lower)?;
    Ok(prefer_uppercase(uppercase, lowercase))
}

fn environment_string(name: &str) -> Result<Option<String>, CoreError> {
    match env::var_os(name) {
        Some(value) => value
            .into_string()
            .map(Some)
            .map_err(|_| CoreError::Proxy(format!("{name} is not valid Unicode"))),
        None => Ok(None),
    }
}

fn prefer_uppercase(uppercase: Option<String>, lowercase: Option<String>) -> Option<String> {
    uppercase.or(lowercase)
}

fn parse_proxy(name: &str, raw: Option<String>) -> Result<Option<Url>, CoreError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }

    let parsed = Url::parse(&raw)
        .map_err(|error| CoreError::Proxy(format!("{name} is not a valid URL: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(CoreError::Proxy(format!(
            "{name} must use http or https, not {}",
            parsed.scheme()
        )));
    }
    if parsed.host_str().is_none() {
        return Err(CoreError::Proxy(format!("{name} must include a host")));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(CoreError::Proxy(format!(
            "{name} must not contain proxy userinfo"
        )));
    }
    Ok(Some(parsed))
}

fn parse_no_proxy(raw: Option<String>) -> Result<Option<String>, CoreError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.chars().any(|character| {
        character.is_control() || character.is_whitespace() || matches!(character, '/' | '\\' | '@')
    }) || raw.contains("://")
    {
        return Err(CoreError::Proxy(
            "NO_PROXY must be a comma-separated host or address list, not a URL".into(),
        ));
    }
    Ok(Some(raw))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        ClientConfiguration, ConnectTimeout, Environment, OperationTimeout, ProxyEnvironment,
        ProxySettings, ResponseBodyLimit, TlsPolicy,
    };
    use crate::errors::{CoreError, ErrorClass};

    #[test]
    fn endpoints_are_fixed_https_urls() {
        assert_eq!(
            Environment::Mainnet.endpoint().unwrap().as_str(),
            "https://www.deribit.com/api/v2"
        );
        assert_eq!(
            Environment::Testnet.endpoint().unwrap().as_str(),
            "https://test.deribit.com/api/v2"
        );
    }

    #[test]
    fn timeout_and_body_budgets_are_finite() {
        assert!(OperationTimeout::new(Duration::ZERO).is_err());
        assert!(OperationTimeout::new(Duration::from_secs(121)).is_err());
        assert!(ConnectTimeout::new(Duration::ZERO).is_err());
        assert!(ResponseBodyLimit::new(0).is_err());
        assert!(ResponseBodyLimit::new(ResponseBodyLimit::MAXIMUM_BYTES + 1).is_err());
    }

    #[test]
    fn configuration_rejects_connect_timeout_beyond_deadline() {
        let error = ClientConfiguration::new(
            Environment::Mainnet,
            OperationTimeout::new(Duration::from_secs(5)).unwrap(),
            ConnectTimeout::new(Duration::from_secs(6)).unwrap(),
            ResponseBodyLimit::default(),
            ProxySettings::default(),
        )
        .unwrap_err();

        assert_eq!(error.class(), ErrorClass::Usage);
        assert!(matches!(error, CoreError::TimeoutPolicy(_)));
    }

    #[test]
    fn proxy_environment_has_explicit_uppercase_precedence() {
        let settings = ProxySettings::from_environment(ProxyEnvironment {
            https_proxy: Some("https://upper.proxy.invalid:8443".into()),
            http_proxy: Some("http://proxy.invalid:8080".into()),
            all_proxy: None,
            no_proxy: Some("localhost,.internal,127.0.0.1".into()),
        })
        .unwrap();

        assert_eq!(
            settings.https_proxy().unwrap().as_str(),
            "https://upper.proxy.invalid:8443/"
        );
        assert_eq!(settings.no_proxy(), Some("localhost,.internal,127.0.0.1"));
    }

    #[test]
    fn uppercase_proxy_spelling_wins_over_lowercase() {
        assert_eq!(
            super::prefer_uppercase(Some("upper".into()), Some("lower".into())),
            Some("upper".into())
        );
        assert_eq!(
            super::prefer_uppercase(None, Some("lower".into())),
            Some("lower".into())
        );
    }

    #[test]
    fn proxy_policy_rejects_userinfo_and_non_http_schemes() {
        let with_userinfo = ProxySettings::from_environment(ProxyEnvironment {
            https_proxy: Some("https://user:password@proxy.invalid".into()),
            ..ProxyEnvironment::default()
        });
        assert!(matches!(with_userinfo, Err(CoreError::Proxy(_))));

        let file_scheme = ProxySettings::from_environment(ProxyEnvironment {
            all_proxy: Some("file:///tmp/proxy".into()),
            ..ProxyEnvironment::default()
        });
        assert!(matches!(file_scheme, Err(CoreError::Proxy(_))));
    }

    #[test]
    fn default_configuration_keeps_rustls_and_mainnet() {
        let configuration = ClientConfiguration::default();
        assert_eq!(configuration.environment(), Environment::Mainnet);
        assert_eq!(configuration.tls_policy(), TlsPolicy::Rustls);
        assert_eq!(
            configuration.response_body_limit().bytes(),
            ResponseBodyLimit::MAXIMUM_BYTES
        );
    }
}
