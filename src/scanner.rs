//! Arbitrage opportunity scanner
//! Detects when YES + NO prices sum to less than $1

use anyhow::Result;
use dashmap::DashMap;
use rust_decimal::Decimal;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::api::{ClobClient, Market, OrderBook, ArbitrageOpportunity};
use crate::config::Config;

/// Scanner for detecting arbitrage opportunities
pub struct ArbitrageScanner {
    client: ClobClient,
    config: Config,
    /// Cache of market data: condition_id -> (yes_token_id, no_token_id)
    market_cache: Arc<DashMap<String, MarketPair>>,
}

/// Cached market pair info
#[derive(Debug, Clone)]
pub struct MarketPair {
    pub condition_id: String,
    pub yes_token_id: String,
    pub no_token_id: String,
    pub description: String,
}

impl ArbitrageScanner {
    pub fn new(client: ClobClient, config: Config) -> Self {
        Self {
            client,
            config,
            market_cache: Arc::new(DashMap::new()),
        }
    }

    /// Refresh the market cache
    pub async fn refresh_markets(&self) -> Result<usize> {
        info!("Refreshing market cache...");

        let markets = if self.config.crypto_only {
            self.client.get_crypto_markets().await?
        } else {
            self.client.get_markets().await?
        };

        let mut count = 0;
        for market in markets.into_iter().take(self.config.max_markets) {
            if let Some(pair) = Self::extract_market_pair(&market) {
                self.market_cache.insert(pair.condition_id.clone(), pair);
                count += 1;
            }
        }

        info!("Cached {} active markets", count);
        Ok(count)
    }

    /// Extract YES/NO token IDs from a market
    fn extract_market_pair(market: &Market) -> Option<MarketPair> {
        if market.tokens.len() != 2 {
            return None;
        }

        let mut yes_token = None;
        let mut no_token = None;

        for token in &market.tokens {
            match token.outcome.to_lowercase().as_str() {
                "yes" => yes_token = Some(token.token_id.clone()),
                "no" => no_token = Some(token.token_id.clone()),
                _ => {}
            }
        }

        match (yes_token, no_token) {
            (Some(yes_id), Some(no_id)) => Some(MarketPair {
                condition_id: market.condition_id.clone(),
                yes_token_id: yes_id,
                no_token_id: no_id,
                description: market.question.clone().unwrap_or_else(|| "Unknown".to_string()),
            }),
            _ => None,
        }
    }

    /// Scan all cached markets for arbitrage opportunities
    pub async fn scan_opportunities(&self) -> Result<Vec<ArbitrageOpportunity>> {
        let mut opportunities = Vec::new();

        // Collect all token IDs for batch request
        let pairs: Vec<MarketPair> = self.market_cache
            .iter()
            .map(|entry| entry.value().clone())
            .collect();

        if pairs.is_empty() {
            warn!("No markets in cache - call refresh_markets first");
            return Ok(opportunities);
        }

        info!("Scanning {} markets...", pairs.len());

        // Batch fetch order books for efficiency
        let all_token_ids: Vec<String> = pairs
            .iter()
            .flat_map(|p| vec![p.yes_token_id.clone(), p.no_token_id.clone()])
            .collect();

        info!("Fetching {} order books...", all_token_ids.len());

        // Log some sample token IDs for debugging
        if let Some(first_pair) = pairs.first() {
            info!("Sample market: {}", first_pair.description);
            info!("  YES token: {}", first_pair.yes_token_id);
            info!("  NO token: {}", first_pair.no_token_id);
        }
        let order_books = self.client.get_order_books(&all_token_ids).await?;
        info!("Got {} order books", order_books.len());

        // Create a map of token_id -> order_book for fast lookup
        let book_map: std::collections::HashMap<String, &OrderBook> = order_books
            .iter()
            .map(|ob| (ob.asset_id.clone(), ob))
            .collect();

        // Check each market pair
        for pair in &pairs {
            if let Some(opp) = self.check_arbitrage(&pair, &book_map) {
                opportunities.push(opp);
            }
        }

        if !opportunities.is_empty() {
            info!("Found {} arbitrage opportunities!", opportunities.len());
        }

        Ok(opportunities)
    }

    /// Check a single market for arbitrage opportunity
    fn check_arbitrage(
        &self,
        pair: &MarketPair,
        book_map: &std::collections::HashMap<String, &OrderBook>,
    ) -> Option<ArbitrageOpportunity> {
        let yes_book = match book_map.get(&pair.yes_token_id) {
            Some(b) => b,
            None => {
                debug!("No YES order book for: {}", &pair.description[..pair.description.len().min(30)]);
                return None;
            }
        };
        let no_book = match book_map.get(&pair.no_token_id) {
            Some(b) => b,
            None => {
                debug!("No NO order book for: {}", &pair.description[..pair.description.len().min(30)]);
                return None;
            }
        };

        // Get best ask prices (cheapest to buy)
        let yes_ask = match yes_book.asks.first() {
            Some(a) => a,
            None => {
                info!("No YES asks for: {} (bids: {})", &pair.description[..pair.description.len().min(30)], yes_book.bids.len());
                return None;
            }
        };
        let no_ask = match no_book.asks.first() {
            Some(a) => a,
            None => {
                info!("No NO asks for: {} (bids: {})", &pair.description[..pair.description.len().min(30)], no_book.bids.len());
                return None;
            }
        };

        let combined_price = yes_ask.price + no_ask.price;
        let profit_per_share = Decimal::ONE - combined_price;

        // Always log prices for debugging (at info level for visibility)
        // Safely truncate to ~35 chars respecting UTF-8 boundaries
        let desc_truncated: String = pair.description.chars().take(35).collect();
        info!(
            "Market: {} | YES: ${} | NO: ${} | Combined: ${:.3} | Spread: {:.4}",
            desc_truncated,
            yes_ask.price,
            no_ask.price,
            combined_price,
            profit_per_share
        );

        // Check if profitable
        if combined_price < self.config.max_combined_price
            && profit_per_share >= self.config.min_profit_threshold
        {
            // Max size is limited by the smaller order book side
            let max_size = yes_ask.size.min(no_ask.size);

            debug!(
                "Arbitrage found: {} - YES@{} + NO@{} = {} (profit: {})",
                pair.description,
                yes_ask.price,
                no_ask.price,
                combined_price,
                profit_per_share
            );

            return Some(ArbitrageOpportunity {
                market_id: pair.condition_id.clone(),
                yes_token_id: pair.yes_token_id.clone(),
                no_token_id: pair.no_token_id.clone(),
                yes_ask_price: yes_ask.price,
                no_ask_price: no_ask.price,
                combined_price,
                profit_per_share,
                max_size,
                timestamp: chrono::Utc::now(),
            });
        }

        None
    }

    /// Scan a single market by ID (for WebSocket-triggered checks)
    pub async fn scan_market(&self, condition_id: &str) -> Result<Option<ArbitrageOpportunity>> {
        let pair = match self.market_cache.get(condition_id) {
            Some(p) => p.clone(),
            None => return Ok(None),
        };

        let token_ids = vec![pair.yes_token_id.clone(), pair.no_token_id.clone()];
        let order_books = self.client.get_order_books(&token_ids).await?;

        let book_map: std::collections::HashMap<String, &OrderBook> = order_books
            .iter()
            .map(|ob| (ob.asset_id.clone(), ob))
            .collect();

        Ok(self.check_arbitrage(&pair, &book_map))
    }

    /// Get all watched token IDs (for WebSocket subscriptions)
    pub fn get_watched_token_ids(&self) -> Vec<String> {
        self.market_cache
            .iter()
            .flat_map(|entry| {
                let pair = entry.value();
                vec![pair.yes_token_id.clone(), pair.no_token_id.clone()]
            })
            .collect()
    }

    /// Get reference to the client
    pub fn client(&self) -> &ClobClient {
        &self.client
    }

    /// Get reference to config
    pub fn config(&self) -> &Config {
        &self.config
    }
}
