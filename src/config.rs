//! Configuration for the arbitrage bot
//! Loads settings from environment variables

use anyhow::{Context, Result};
use rust_decimal::Decimal;
use std::str::FromStr;

/// Bot configuration
#[derive(Debug, Clone)]
pub struct Config {
    /// Ethereum private key for signing
    pub private_key: String,

    /// Maximum combined price to trigger arbitrage (e.g., 0.99 = 99¢)
    pub max_combined_price: Decimal,

    /// Minimum profit per share to execute (e.g., 0.01 = 1¢)
    pub min_profit_threshold: Decimal,

    /// Order size in USDC
    pub order_size: Decimal,

    /// Polling interval in milliseconds
    pub poll_interval_ms: u64,

    /// Whether to use WebSocket for real-time updates
    pub use_websocket: bool,

    /// Maximum concurrent markets to scan
    pub max_markets: usize,

    /// Only trade crypto markets (15-min expiry)
    pub crypto_only: bool,

    /// Dry run mode - detect but don't execute
    pub dry_run: bool,
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok(); // Load .env file if present

        let private_key = std::env::var("POLYMARKET_PRIVATE_KEY")
            .context("POLYMARKET_PRIVATE_KEY not set")?;

        let max_combined_price = std::env::var("MAX_COMBINED_PRICE")
            .unwrap_or_else(|_| "0.99".to_string());
        let max_combined_price = Decimal::from_str(&max_combined_price)
            .context("Invalid MAX_COMBINED_PRICE")?;

        let min_profit_threshold = std::env::var("MIN_PROFIT_THRESHOLD")
            .unwrap_or_else(|_| "0.005".to_string()); // 0.5¢ default
        let min_profit_threshold = Decimal::from_str(&min_profit_threshold)
            .context("Invalid MIN_PROFIT_THRESHOLD")?;

        let order_size = std::env::var("ORDER_SIZE")
            .unwrap_or_else(|_| "10.0".to_string()); // $10 default
        let order_size = Decimal::from_str(&order_size)
            .context("Invalid ORDER_SIZE")?;

        let poll_interval_ms = std::env::var("POLL_INTERVAL_MS")
            .unwrap_or_else(|_| "2000".to_string()) // 2 seconds default
            .parse()
            .context("Invalid POLL_INTERVAL_MS")?;

        let use_websocket = std::env::var("USE_WEBSOCKET")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);

        let max_markets = std::env::var("MAX_MARKETS")
            .unwrap_or_else(|_| "50".to_string())
            .parse()
            .context("Invalid MAX_MARKETS")?;

        let crypto_only = std::env::var("CRYPTO_ONLY")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true); // Default to crypto markets for 15-min expiry

        let dry_run = std::env::var("DRY_RUN")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true); // Default to dry run for safety

        Ok(Self {
            private_key,
            max_combined_price,
            min_profit_threshold,
            order_size,
            poll_interval_ms,
            use_websocket,
            max_markets,
            crypto_only,
            dry_run,
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            private_key: String::new(),
            max_combined_price: Decimal::from_str("0.99").unwrap(),
            min_profit_threshold: Decimal::from_str("0.005").unwrap(),
            order_size: Decimal::from_str("10.0").unwrap(),
            poll_interval_ms: 2000,
            use_websocket: true,
            max_markets: 50,
            crypto_only: true,
            dry_run: true,
        }
    }
}
