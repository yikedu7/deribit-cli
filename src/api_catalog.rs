use std::collections::{BTreeMap, BTreeSet};

use clap::ArgMatches;
use serde::Deserialize;
use serde_json::Value;

use crate::ManifestRegistry;
use crate::api_application::Failure;
use crate::api_commands::named_command;

pub(crate) const CATALOG_JSON: &str = include_str!("../assets/api-catalog.json");

const TESTNET_COVERAGE: &str = include_str!("../assets/api-testnet-coverage.json");

#[derive(Deserialize)]
struct TestnetCoverage {
    as_of: String,
    rpc_verified: BTreeSet<String>,
    subscription_acknowledged: BTreeSet<String>,
    subscription_event_received: BTreeSet<String>,
}

pub(crate) fn inspect(registry: &ManifestRegistry, options: &ArgMatches) -> Result<Value, Failure> {
    let mut catalog: Value = serde_json::from_str(CATALOG_JSON).expect("embedded API catalog");
    let coverage: TestnetCoverage =
        serde_json::from_str(TESTNET_COVERAGE).expect("embedded Testnet coverage");
    catalog["testnet_evidence_as_of"] = Value::String(coverage.as_of.clone());
    for row in catalog["methods"].as_array_mut().expect("catalog methods") {
        let method = row["method"].as_str().expect("method name");
        let verified = coverage.rpc_verified.contains(method);
        if let Some(command) = named_command(registry, method) {
            row["cli_command"] = serde_json::json!(command);
        }
        row["testnet_verified"] = Value::Bool(verified);
    }
    for row in catalog["channels"]
        .as_array_mut()
        .expect("catalog channels")
    {
        let template = row["channel"].as_str().expect("channel template");
        let acknowledged = coverage.subscription_acknowledged.contains(template);
        let event = coverage.subscription_event_received.contains(template);
        row["testnet_acknowledged"] = Value::Bool(acknowledged);
        row["testnet_event_received"] = Value::Bool(event);
    }
    let (rows, field, name, missing) = match options.subcommand() {
        None => return Ok(catalog),
        Some(("method", options)) => (
            "methods",
            "method",
            options.get_one::<String>("method").unwrap(),
            "method is absent from the pinned Deribit API catalog",
        ),
        Some(("channel", options)) => (
            "channels",
            "channel",
            options.get_one::<String>("channel").unwrap(),
            "channel template is absent from the pinned Deribit API catalog",
        ),
        _ => unreachable!("only declared catalog commands are accepted"),
    };
    let mut row = catalog[rows]
        .as_array_mut()
        .expect("catalog entries")
        .iter_mut()
        .find(|entry| entry[field].as_str() == Some(name))
        .map(Value::take)
        .ok_or(Failure::Input(missing))?;
    row["testnet_evidence_as_of"] = Value::String(coverage.as_of);
    Ok(row)
}

#[derive(Deserialize)]
pub(crate) struct Catalog {
    methods: Vec<Method>,
    channels: Vec<Channel>,
}

#[derive(Deserialize)]
pub(crate) struct Method {
    #[serde(rename = "method")]
    pub name: String,
    pub auth: bool,
    pub effect: Effect,
    pub transports: Vec<String>,
    pub documented_scope: Option<String>,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Effect {
    Authentication,
    Read,
    Session,
    Write,
    Special,
}

#[derive(Deserialize)]
pub(crate) struct Channel {
    pub channel: String,
    pub auth: bool,
    pub auth_for_raw: bool,
    pub documented_scope: Option<String>,
    parameters: BTreeMap<String, Option<Vec<String>>>,
}

impl Catalog {
    pub fn load() -> Self {
        serde_json::from_str(CATALOG_JSON).expect("embedded API catalog")
    }

    pub fn method(&self, name: &str) -> Option<&Method> {
        let index = self
            .methods
            .binary_search_by(|item| item.name.as_str().cmp(name))
            .ok()?;
        Some(&self.methods[index])
    }

    pub fn channel(&self, concrete: &str) -> Option<&Channel> {
        self.channels.iter().find(|row| row.matches(concrete))
    }
}

impl Channel {
    fn matches(&self, concrete: &str) -> bool {
        let mut segments = concrete.split('.');
        for template in self.channel.split('.') {
            let Some(segment) = segments.next() else {
                return false;
            };
            if let Some(parameter) = template
                .strip_prefix('(')
                .and_then(|part| part.strip_suffix(')'))
            {
                if segment.is_empty()
                    || !segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                {
                    return false;
                }
                // Instruments and currencies change without a new API release.
                // The service validates their current domain. Interval and depth
                // are part of the pinned channel syntax.
                if matches!(parameter, "interval" | "depth" | "group" | "resolution")
                    && self
                        .parameters
                        .get(parameter)
                        .and_then(Option::as_ref)
                        .is_some_and(|values| !values.iter().any(|value| value == segment))
                {
                    return false;
                }
            } else if segment != template {
                return false;
            }
        }
        segments.next().is_none()
    }
}
