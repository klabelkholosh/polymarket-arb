//! Polymarket CLOB API data types
//! Optimized for zero-copy deserialization where possible

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Market data from the CLOB API
#[derive(Debug, Clone, Deserialize)]
pub struct Market {
    pub condition_id: String,
    pub question_id: String,
    pub tokens: Vec<Token>,
    pub minimum_order_size: Decimal,
    pub minimum_tick_size: Decimal,
    pub description: Option<String>,
    pub category: Option<String>,
    pub end_date_iso: Option<String>,
    pub game_start_time: Option<String>,
    pub question: Option<String>,
    pub market_slug: Option<String>,
    pub active: bool,
    pub closed: bool,
    pub accepting_orders: bool,
}

/// Token representing YES or NO outcome
#[derive(Debug, Clone, Deserialize)]
pub struct Token {
    pub token_id: String,
    pub outcome: String, // "Yes" or "No"
    pub price: Option<Decimal>,
    pub winner: bool,
}

/// Order book summary for a token
#[derive(Debug, Clone, Deserialize)]
pub struct OrderBook {
    pub market: String,
    pub asset_id: String,
    pub bids: Vec<OrderBookEntry>,
    pub asks: Vec<OrderBookEntry>,
    pub hash: String,
    pub timestamp: String,
}

/// Single order book entry (price level)
#[derive(Debug, Clone, Deserialize)]
pub struct OrderBookEntry {
    pub price: Decimal,
    pub size: Decimal,
}

/// Price response from the API
#[derive(Debug, Clone, Deserialize)]
pub struct PriceResponse {
    pub price: Decimal,
}

/// Midpoint price response
#[derive(Debug, Clone, Deserialize)]
pub struct MidpointResponse {
    pub mid: Decimal,
}

/// Multiple order books response
#[derive(Debug, Clone, Deserialize)]
pub struct BooksResponse(pub Vec<OrderBook>);

/// API credentials for L2 authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiCredentials {
    #[serde(rename = "apiKey")]
    pub api_key: String,
    pub secret: String,
    pub passphrase: String,
}

/// Order side
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Buy,
    Sell,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

/// Order type
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderType {
    Gtc, // Good till cancelled
    Fok, // Fill or kill - critical for arb to avoid partial fills
    Ioc, // Immediate or cancel
}

/// Signed order to be submitted
#[derive(Debug, Clone, Serialize)]
pub struct SignedOrder {
    pub order: OrderData,
    pub signature: String,
    #[serde(rename = "signatureType")]
    pub signature_type: u8,
}

/// Order data for signing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderData {
    pub salt: String,
    pub maker: String,
    pub signer: String,
    #[serde(rename = "taker")]
    pub taker: String,
    #[serde(rename = "tokenId")]
    pub token_id: String,
    #[serde(rename = "makerAmount")]
    pub maker_amount: String,
    #[serde(rename = "takerAmount")]
    pub taker_amount: String,
    pub expiration: String,
    pub nonce: String,
    #[serde(rename = "feeRateBps")]
    pub fee_rate_bps: String,
    pub side: Side,
    #[serde(rename = "signatureType")]
    pub signature_type: u8,
}

/// Order placement request
#[derive(Debug, Clone, Serialize)]
pub struct PlaceOrderRequest {
    pub order: SignedOrder,
    #[serde(rename = "orderType")]
    pub order_type: OrderType,
}

/// Order response from API
#[derive(Debug, Clone, Deserialize)]
pub struct OrderResponse {
    pub success: bool,
    #[serde(rename = "errorMsg")]
    pub error_msg: Option<String>,
    #[serde(rename = "orderID")]
    pub order_id: Option<String>,
    #[serde(rename = "transactionsHashes")]
    pub transaction_hashes: Option<Vec<String>>,
}

/// WebSocket subscription message
#[derive(Debug, Clone, Serialize)]
pub struct WsSubscribe {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub assets_ids: Vec<String>,
}

/// WebSocket price update event
#[derive(Debug, Clone, Deserialize)]
pub struct WsPriceChange {
    pub asset_id: String,
    pub price: Decimal,
    pub timestamp: String,
}

/// WebSocket book update event
#[derive(Debug, Clone, Deserialize)]
pub struct WsBookUpdate {
    pub asset_id: String,
    pub market: String,
    pub bids: Vec<OrderBookEntry>,
    pub asks: Vec<OrderBookEntry>,
    pub timestamp: String,
    pub hash: String,
}

/// Generic WebSocket message
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "event_type")]
pub enum WsEvent {
    #[serde(rename = "book")]
    Book(WsBookUpdate),
    #[serde(rename = "price_change")]
    PriceChange(WsPriceChange),
    #[serde(rename = "last_trade_price")]
    LastTradePrice { asset_id: String, price: Decimal },
    #[serde(rename = "tick_size_change")]
    TickSizeChange { asset_id: String, tick_size: Decimal },
}

/// Arbitrage opportunity
#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    pub market_id: String,
    pub yes_token_id: String,
    pub no_token_id: String,
    pub yes_ask_price: Decimal,
    pub no_ask_price: Decimal,
    pub combined_price: Decimal,
    pub profit_per_share: Decimal,
    pub max_size: Decimal,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ArbitrageOpportunity {
    /// Calculate expected profit for a given size
    pub fn expected_profit(&self, size: Decimal) -> Decimal {
        self.profit_per_share * size
    }
}

/// Markets list response with pagination
#[derive(Debug, Clone, Deserialize)]
pub struct MarketsResponse {
    pub data: Vec<Market>,
    pub next_cursor: Option<String>,
}
