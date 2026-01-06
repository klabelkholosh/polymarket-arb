//! High-performance Polymarket CLOB API client
//! Uses the official Polymarket SDK for authentication

use anyhow::{Context, Result};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer as AlloySigner;
use chrono::{DateTime, Utc};
use polymarket_client_sdk::auth::{state::Authenticated, Normal};
use polymarket_client_sdk::clob::{
    Client as PolyClient, Config as PolyConfig,
    types::{OrderBookSummaryRequestBuilder, Amount, Side as PolySide, PostOrderResponse},
};
use polymarket_client_sdk::error::Error as PolyError;
use polymarket_client_sdk::POLYGON;
use rust_decimal::Decimal;
use std::str::FromStr;
use tracing::{debug, error, info, warn};

use super::types::*;

/// CLOB API endpoints
const CLOB_HOST: &str = "https://clob.polymarket.com";

/// High-performance CLOB client using official SDK
pub struct ClobClient {
    /// The authenticated Polymarket client
    client: PolyClient<Authenticated<Normal>>,
    /// Signer for order signing (PrivateKeySigner = LocalSigner<SigningKey<Secp256k1>>)
    signer: PrivateKeySigner,
    /// Wallet address
    address: String,
}

impl ClobClient {
    /// Create new client with authentication
    pub async fn new(private_key: &str) -> Result<Self> {
        info!("Initializing Polymarket client with official SDK...");

        // Create signer with alloy (PrivateKeySigner is a type alias for LocalSigner<SigningKey>)
        let signer: PrivateKeySigner = private_key
            .parse()
            .context("Failed to parse private key")?;
        let signer = signer.with_chain_id(Some(POLYGON));

        let address = format!("{:?}", signer.address());
        info!("Wallet address: {}", address);

        // Create and authenticate client using official SDK
        let client = PolyClient::new(CLOB_HOST, PolyConfig::default())
            .context("Failed to create Polymarket client")?
            .authentication_builder(&signer)
            .authenticate()
            .await
            .context("Failed to authenticate with Polymarket")?;

        info!("Successfully authenticated with Polymarket API");

        Ok(Self { client, signer, address })
    }

    /// Get wallet address
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Get all active markets
    pub async fn get_markets(&self) -> Result<Vec<Market>> {
        info!("Fetching markets from API...");

        // Use sampling_markets which returns actively traded markets
        let markets_response = self.client.sampling_markets(None).await
            .context("Failed to fetch sampling markets")?;

        info!("API returned {} markets in this page", markets_response.data.len());

        // Convert SDK response to our types (Page has .data field)
        // Filter to only include active, non-closed markets accepting orders
        let markets: Vec<Market> = markets_response.data
            .into_iter()
            .filter(|m| m.active && !m.closed && m.accepting_orders)
            .map(|m| Market {
                condition_id: m.condition_id,
                question_id: m.question_id,
                tokens: m.tokens.into_iter().map(|t| Token {
                    token_id: t.token_id,
                    outcome: t.outcome,
                    price: Some(t.price),
                    winner: t.winner,
                }).collect(),
                minimum_order_size: m.minimum_order_size,
                minimum_tick_size: m.minimum_tick_size,
                description: Some(m.description),
                category: None, // SDK doesn't have this field directly
                end_date_iso: m.end_date_iso.map(|d: DateTime<Utc>| d.to_rfc3339()),
                game_start_time: m.game_start_time.map(|d: DateTime<Utc>| d.to_rfc3339()),
                question: Some(m.question),
                market_slug: Some(m.market_slug),
                active: m.active,
                closed: m.closed,
                accepting_orders: m.accepting_orders,
            })
            .collect();

        info!("Fetched {} active markets (filtered from response)", markets.len());
        Ok(markets)
    }

    /// Get crypto markets (filter by question for crypto-related content)
    pub async fn get_crypto_markets(&self) -> Result<Vec<Market>> {
        let markets = self.get_markets().await?;

        Ok(markets
            .into_iter()
            .filter(|m| {
                m.active
                    && !m.closed
                    && m.accepting_orders
                    && m.question
                        .as_ref()
                        .map(|q| {
                            let q_lower = q.to_lowercase();
                            q_lower.contains("bitcoin")
                                || q_lower.contains("btc")
                                || q_lower.contains("ethereum")
                                || q_lower.contains("eth")
                                || q_lower.contains("crypto")
                        })
                        .unwrap_or(false)
            })
            .collect())
    }

    /// Get multiple order books (fetch in parallel)
    pub async fn get_order_books(&self, token_ids: &[String]) -> Result<Vec<OrderBook>> {
        if token_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Build requests for each token using the builder pattern
        let requests: Vec<_> = token_ids
            .iter()
            .filter_map(|id| {
                OrderBookSummaryRequestBuilder::default()
                    .token_id(id.clone())
                    .build()
                    .ok()
            })
            .collect();

        if requests.is_empty() {
            return Ok(Vec::new());
        }

        // Fetch order books individually using GET /book (more reliable than batch POST /books)
        let futures: Vec<_> = requests.iter().map(|req| {
            self.client.order_book(req)
        }).collect();

        let results = futures_util::future::join_all(futures).await;

        let mut books = Vec::new();
        let mut errors = 0;
        for result in results {
            match result {
                Ok(b) => {
                    books.push(OrderBook {
                        market: b.market,
                        asset_id: b.asset_id,
                        bids: b.bids.into_iter().map(|e| OrderBookEntry {
                            price: e.price,
                            size: e.size,
                        }).collect(),
                        asks: b.asks.into_iter().map(|e| OrderBookEntry {
                            price: e.price,
                            size: e.size,
                        }).collect(),
                        hash: b.hash.unwrap_or_default(),
                        timestamp: b.timestamp.to_rfc3339(),
                    });
                },
                Err(e) => {
                    errors += 1;
                    // Log first few errors at info level to help debug
                    if errors <= 3 {
                        info!("Order book fetch failed: {}", e);
                    }
                }
            }
        }

        if errors > 0 {
            debug!("Failed to fetch {} order books", errors);
        }

        Ok(books)
    }

    /// Execute arbitrage: buy YES + NO simultaneously using parallel execution
    ///
    /// This is the critical path - both orders are submitted in parallel using tokio::join!
    /// to minimize the time window where prices could move against us.
    pub async fn execute_arbitrage(
        &self,
        opportunity: &ArbitrageOpportunity,
        size: Decimal,
    ) -> Result<(OrderResponse, OrderResponse)> {
        info!(
            "Executing PARALLEL arbitrage on market {} - profit per share: ${}",
            opportunity.market_id,
            opportunity.profit_per_share
        );

        // Calculate the USDC amount based on size and prices
        // We're buying `size` shares of each outcome
        let yes_usdc = size * opportunity.yes_ask_price;
        let no_usdc = size * opportunity.no_ask_price;

        info!(
            "Placing parallel orders: YES ${} @ {} | NO ${} @ {}",
            yes_usdc, opportunity.yes_ask_price,
            no_usdc, opportunity.no_ask_price
        );

        // Build both orders in parallel (order building is async)
        let (yes_order_result, no_order_result) = tokio::join!(
            self.build_and_sign_order(
                &opportunity.yes_token_id,
                yes_usdc,
                PolySide::Buy,
            ),
            self.build_and_sign_order(
                &opportunity.no_token_id,
                no_usdc,
                PolySide::Buy,
            )
        );

        // Check if both orders were built successfully
        let yes_signed = yes_order_result.context("Failed to build YES order")?;
        let no_signed = no_order_result.context("Failed to build NO order")?;

        info!("Both orders built and signed, submitting in parallel...");

        // Submit both orders in parallel - this is the critical section!
        let (yes_response, no_response) = tokio::join!(
            self.client.post_order(yes_signed),
            self.client.post_order(no_signed)
        );

        // Convert responses
        let yes_result = self.convert_response(yes_response, "YES");
        let no_result = self.convert_response(no_response, "NO");

        // Log results
        if yes_result.success && no_result.success {
            info!("Both orders submitted successfully!");
        } else {
            warn!(
                "Order submission results - YES: {} | NO: {}",
                if yes_result.success { "OK" } else { "FAILED" },
                if no_result.success { "OK" } else { "FAILED" }
            );

            // Handle partial fill scenario
            if yes_result.success != no_result.success {
                self.handle_partial_execution(&yes_result, &no_result, opportunity).await;
            }
        }

        Ok((yes_result, no_result))
    }

    /// Build and sign a market order
    async fn build_and_sign_order(
        &self,
        token_id: &str,
        usdc_amount: Decimal,
        side: PolySide,
    ) -> Result<polymarket_client_sdk::clob::types::SignedOrder> {
        // Build a market order (FOK - Fill or Kill for immediate execution)
        let amount = Amount::usdc(usdc_amount)
            .context("Failed to create USDC amount")?;

        let order = self.client
            .market_order()
            .token_id(token_id)
            .amount(amount)
            .side(side)
            .build()
            .await
            .context("Failed to build market order")?;

        // Sign the order
        let signed = self.client
            .sign(&self.signer, order)
            .await
            .context("Failed to sign order")?;

        Ok(signed)
    }

    /// Convert SDK response to our OrderResponse type
    /// Note: post_order returns Vec<PostOrderResponse> but we only send one order
    fn convert_response(
        &self,
        response: std::result::Result<Vec<PostOrderResponse>, PolyError>,
        side_name: &str,
    ) -> OrderResponse {
        match response {
            Ok(responses) => {
                // We send one order at a time, so expect one response
                if let Some(resp) = responses.into_iter().next() {
                    if resp.success {
                        info!(
                            "{} order filled: {} shares @ order_id={}",
                            side_name,
                            resp.making_amount,
                            resp.order_id
                        );
                    } else {
                        warn!(
                            "{} order failed: {:?}",
                            side_name,
                            resp.error_msg
                        );
                    }
                    OrderResponse {
                        success: resp.success,
                        error_msg: resp.error_msg,
                        order_id: Some(resp.order_id),
                        transaction_hashes: if resp.transaction_hashes.is_empty() {
                            None
                        } else {
                            Some(resp.transaction_hashes)
                        },
                    }
                } else {
                    error!("{} order returned empty response", side_name);
                    OrderResponse {
                        success: false,
                        error_msg: Some("Empty response from server".to_string()),
                        order_id: None,
                        transaction_hashes: None,
                    }
                }
            }
            Err(e) => {
                error!("{} order error: {}", side_name, e);
                OrderResponse {
                    success: false,
                    error_msg: Some(e.to_string()),
                    order_id: None,
                    transaction_hashes: None,
                }
            }
        }
    }

    /// Handle partial execution scenario (one order succeeded, one failed)
    ///
    /// In arbitrage, if only one side executes, we're exposed to market risk.
    /// This function attempts to mitigate by either:
    /// 1. Retrying the failed order
    /// 2. Unwinding the successful order if retry fails
    async fn handle_partial_execution(
        &self,
        yes_result: &OrderResponse,
        no_result: &OrderResponse,
        opportunity: &ArbitrageOpportunity,
    ) {
        warn!("PARTIAL EXECUTION DETECTED - attempting recovery");

        // Determine which side failed
        let (failed_side, failed_token) = if yes_result.success {
            ("NO", &opportunity.no_token_id)
        } else {
            ("YES", &opportunity.yes_token_id)
        };

        warn!(
            "{} order failed, {} order succeeded. Manual intervention may be required.",
            failed_side,
            if failed_side == "NO" { "YES" } else { "NO" }
        );

        // Log the situation for manual review
        error!(
            "ALERT: Partial arbitrage execution on market {}. {} position is unhedged!",
            opportunity.market_id,
            if failed_side == "NO" { "YES" } else { "NO" }
        );

        // In a production system, we would:
        // 1. Retry the failed order with updated prices
        // 2. If retry fails, place a market order to unwind the successful position
        // 3. Send alerts to monitoring systems
        //
        // For now, we just log the issue prominently
    }
}
