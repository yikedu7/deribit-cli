use std::collections::BTreeMap;

use serde::Deserialize;

pub(crate) const CATALOG_JSON: &str = include_str!("../assets/catalog.json");

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
