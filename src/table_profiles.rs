//! Frozen table profile registry and explicit display metadata.

/// Every v1 named profile plus the mandatory generic fallback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableProfile {
    Supporting,
    Announcement,
    Index,
    Ticker,
    OrderBook,
    Instruments,
    Options,
    Volatility,
    Funding,
    Combo,
    Generic,
}

/// Display semantics that must be declared by a profile rather than guessed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldKind {
    Text,
    Identifier,
    FreeText,
    Enum,
    Number,
    Price,
    PercentValue,
    PercentFraction,
    TimestampMillis,
    EmbeddedJson,
}

impl TableProfile {
    /// The ten frozen named profiles. Generic is a fallback, not a manifest ID.
    pub const NAMED: [Self; 10] = [
        Self::Supporting,
        Self::Announcement,
        Self::Index,
        Self::Ticker,
        Self::OrderBook,
        Self::Instruments,
        Self::Options,
        Self::Volatility,
        Self::Funding,
        Self::Combo,
    ];

    /// Resolves a manifest profile ID, using generic only for an explicit null.
    pub fn from_manifest(value: Option<&str>) -> Option<Self> {
        match value {
            Some("supporting") => Some(Self::Supporting),
            Some("announcement") => Some(Self::Announcement),
            Some("index") => Some(Self::Index),
            Some("ticker") => Some(Self::Ticker),
            Some("order_book") => Some(Self::OrderBook),
            Some("instruments") => Some(Self::Instruments),
            Some("options") => Some(Self::Options),
            Some("volatility") => Some(Self::Volatility),
            Some("funding") => Some(Self::Funding),
            Some("combo") => Some(Self::Combo),
            None => Some(Self::Generic),
            Some(_) => None,
        }
    }

    /// Stable profile label used in titles and golden snapshots.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supporting => "supporting",
            Self::Announcement => "announcement",
            Self::Index => "index",
            Self::Ticker => "ticker",
            Self::OrderBook => "order_book",
            Self::Instruments => "instruments",
            Self::Options => "options",
            Self::Volatility => "volatility",
            Self::Funding => "funding",
            Self::Combo => "combo",
            Self::Generic => "generic",
        }
    }

    /// Preferred raw field paths. Missing fields are not invented for nonempty data.
    pub const fn preferred_columns(self) -> &'static [&'static str] {
        match self {
            Self::Supporting => &["status", "version", "server_time", "maintenance", "message"],
            Self::Announcement => &[
                "id",
                "type",
                "title",
                "published_at",
                "updated_at",
                "url",
                "body",
            ],
            Self::Index => &[
                "index_name",
                "index_price",
                "estimated_delivery_price",
                "timestamp",
            ],
            Self::Ticker => &[
                "instrument_name",
                "last_price",
                "best_bid_price",
                "best_ask_price",
                "mark_price",
                "index_price",
                "mark_iv",
                "open_interest",
                "stats.volume",
                "stats.price_change",
                "greeks.delta",
                "greeks.gamma",
                "greeks.vega",
                "greeks.theta",
                "timestamp",
            ],
            Self::OrderBook => &["timestamp", "change_id", "state"],
            Self::Instruments => &[
                "instrument_name",
                "kind",
                "base_currency",
                "quote_currency",
                "settlement_currency",
                "is_active",
                "expiration_timestamp",
                "strike",
                "option_type",
                "contract_size",
                "min_trade_amount",
                "tick_size",
                "creation_timestamp",
            ],
            Self::Options => &[
                "instrument_name",
                "expiration_timestamp",
                "strike",
                "option_type",
                "bid_price",
                "ask_price",
                "mark_price",
                "mark_iv",
                "greeks.delta",
                "greeks.gamma",
                "greeks.vega",
                "greeks.theta",
                "open_interest",
                "stats.volume",
            ],
            Self::Volatility => &["timestamp", "value", "resolution"],
            Self::Funding => &[
                "timestamp",
                "funding_rate",
                "funding_8h",
                "index_price",
                "mark_price",
            ],
            Self::Combo => &[
                "instrument_name",
                "state",
                "creation_timestamp",
                "expiration_timestamp",
                "legs",
                "direction",
                "ratio",
            ],
            Self::Generic => &[],
        }
    }

    /// Returns explicit field semantics. Generic deliberately has no heuristics.
    pub fn field_kind(self, path: &str) -> FieldKind {
        if self == Self::Generic {
            return FieldKind::Text;
        }
        if self.timestamp_fields().contains(&path) {
            return FieldKind::TimestampMillis;
        }
        if self.price_fields().contains(&path) {
            return FieldKind::Price;
        }
        if self.percent_value_fields().contains(&path) {
            return FieldKind::PercentValue;
        }
        if self.percent_fraction_fields().contains(&path) {
            return FieldKind::PercentFraction;
        }
        if self.identifier_fields().contains(&path) {
            return FieldKind::Identifier;
        }
        if self.free_text_fields().contains(&path) {
            return FieldKind::FreeText;
        }
        if self.enum_fields().contains(&path) {
            return FieldKind::Enum;
        }
        if self.embedded_json_fields().contains(&path) {
            return FieldKind::EmbeddedJson;
        }
        if self.number_fields().contains(&path) {
            return FieldKind::Number;
        }
        FieldKind::Text
    }

    fn timestamp_fields(self) -> &'static [&'static str] {
        match self {
            Self::Supporting => &["server_time"],
            Self::Announcement => &["published_at", "updated_at"],
            Self::Index | Self::Ticker | Self::OrderBook | Self::Volatility | Self::Funding => {
                &["timestamp"]
            }
            Self::Instruments | Self::Options | Self::Combo => {
                &["expiration_timestamp", "creation_timestamp"]
            }
            Self::Generic => &[],
        }
    }

    fn price_fields(self) -> &'static [&'static str] {
        match self {
            Self::Index => &["index_price", "estimated_delivery_price"],
            Self::Ticker => &[
                "last_price",
                "best_bid_price",
                "best_ask_price",
                "mark_price",
                "index_price",
            ],
            Self::OrderBook => &["price"],
            Self::Instruments => &["strike", "tick_size"],
            Self::Options => &["strike", "bid_price", "ask_price", "mark_price"],
            Self::Funding => &["index_price", "mark_price"],
            _ => &[],
        }
    }

    fn percent_value_fields(self) -> &'static [&'static str] {
        match self {
            Self::Ticker | Self::Options => &["mark_iv"],
            Self::Volatility => &["value"],
            _ => &[],
        }
    }

    fn percent_fraction_fields(self) -> &'static [&'static str] {
        match self {
            Self::Funding => &["funding_rate", "funding_8h"],
            _ => &[],
        }
    }

    fn identifier_fields(self) -> &'static [&'static str] {
        match self {
            Self::Announcement => &["id", "url"],
            Self::Index => &["index_name"],
            Self::Ticker | Self::Instruments | Self::Options | Self::Combo => &["instrument_name"],
            _ => &[],
        }
    }

    fn free_text_fields(self) -> &'static [&'static str] {
        match self {
            Self::Supporting => &["message"],
            Self::Announcement => &["title", "body"],
            _ => &[],
        }
    }

    fn enum_fields(self) -> &'static [&'static str] {
        match self {
            Self::Announcement => &["type"],
            Self::OrderBook => &["state", "side"],
            Self::Instruments | Self::Options => &["kind", "option_type"],
            Self::Combo => &["state", "direction"],
            _ => &[],
        }
    }

    fn embedded_json_fields(self) -> &'static [&'static str] {
        match self {
            Self::Combo => &["legs"],
            _ => &[],
        }
    }

    fn number_fields(self) -> &'static [&'static str] {
        match self {
            Self::Ticker | Self::Options => &[
                "open_interest",
                "stats.volume",
                "stats.price_change",
                "greeks.delta",
                "greeks.gamma",
                "greeks.vega",
                "greeks.theta",
            ],
            Self::OrderBook => &["change_id", "amount", "count"],
            Self::Instruments => &["contract_size", "min_trade_amount"],
            Self::Volatility => &["resolution"],
            Self::Combo => &["ratio"],
            _ => &[],
        }
    }
}
