//! Polymarket Arbitrage Bot
//!
//! Detects and executes arbitrage opportunities when YES + NO < $1
//! Targets 15-minute crypto markets for fast resolution

mod api;
mod config;
mod scanner;
mod websocket;

use anyhow::Result;
use rust_decimal::Decimal;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn, Level};
use tracing_subscriber::EnvFilter;

use api::{ClobClient, ArbitrageOpportunity};
use config::Config;
use scanner::ArbitrageScanner;
use websocket::{WsClient, PriceUpdate};

/// Stats tracking for the bot
#[derive(Debug, Default)]
struct BotStats {
    opportunities_found: u64,
    trades_executed: u64,
    trades_successful: u64,
    total_profit: Decimal,
    scans_completed: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive(Level::INFO.into())
        )
        .init();

    info!("===========================================");
    info!("   Polymarket Arbitrage Bot v0.1.0");
    info!("===========================================");

    // Load configuration
    let config = Config::from_env()?;

    info!("Configuration:");
    info!("  Max combined price: {}", config.max_combined_price);
    info!("  Min profit threshold: {}", config.min_profit_threshold);
    info!("  Order size: ${}", config.order_size);
    info!("  Poll interval: {}ms", config.poll_interval_ms);
    info!("  Crypto only: {}", config.crypto_only);
    info!("  Dry run: {}", config.dry_run);

    if config.dry_run {
        warn!("*** DRY RUN MODE - No trades will be executed ***");
    }

    // Create API client with authentication (using official Polymarket SDK)
    let client = ClobClient::new(&config.private_key).await?;
    info!("Wallet address: {}", client.address());

    // Create scanner
    let scanner = Arc::new(ArbitrageScanner::new(client, config.clone()));

    // Initial market refresh
    let market_count = scanner.refresh_markets().await?;
    info!("Loaded {} markets to monitor", market_count);

    if market_count == 0 {
        error!("No markets found to monitor. Exiting.");
        return Ok(());
    }

    // Stats tracking
    let stats = Arc::new(RwLock::new(BotStats::default()));

    // Decide on strategy: WebSocket or Polling
    if config.use_websocket {
        run_websocket_mode(scanner, stats, config).await
    } else {
        run_polling_mode(scanner, stats, config).await
    }
}

/// Run the bot in polling mode (1-3 second intervals)
async fn run_polling_mode(
    scanner: Arc<ArbitrageScanner>,
    stats: Arc<RwLock<BotStats>>,
    config: Config,
) -> Result<()> {
    info!("Starting in POLLING mode ({}ms interval)", config.poll_interval_ms);

    let poll_interval = Duration::from_millis(config.poll_interval_ms);
    let mut refresh_counter = 0u64;

    loop {
        let scan_start = std::time::Instant::now();

        // Refresh markets every 100 scans (roughly every 3-5 minutes)
        refresh_counter += 1;
        if refresh_counter % 100 == 0 {
            if let Err(e) = scanner.refresh_markets().await {
                warn!("Failed to refresh markets: {}", e);
            }
        }

        // Scan for opportunities
        match scanner.scan_opportunities().await {
            Ok(opportunities) => {
                let mut stats_guard = stats.write().await;
                stats_guard.scans_completed += 1;

                for opp in opportunities {
                    stats_guard.opportunities_found += 1;
                    handle_opportunity(&opp, scanner.clone(), &mut stats_guard, &config).await;
                }
            }
            Err(e) => {
                warn!("Scan failed: {}", e);
            }
        }

        let scan_duration = scan_start.elapsed();
        debug!("Scan completed in {:?}", scan_duration);

        // Print stats every 50 scans
        if refresh_counter % 50 == 0 {
            let stats_guard = stats.read().await;
            info!(
                "Stats: {} scans | {} opportunities | {} trades ({} successful) | ${} profit",
                stats_guard.scans_completed,
                stats_guard.opportunities_found,
                stats_guard.trades_executed,
                stats_guard.trades_successful,
                stats_guard.total_profit
            );
        }

        // Sleep for remaining interval
        if scan_duration < poll_interval {
            tokio::time::sleep(poll_interval - scan_duration).await;
        }
    }
}

/// Run the bot in WebSocket mode (real-time updates)
async fn run_websocket_mode(
    scanner: Arc<ArbitrageScanner>,
    stats: Arc<RwLock<BotStats>>,
    config: Config,
) -> Result<()> {
    info!("Starting in WEBSOCKET mode (real-time updates)");

    // Track prices in memory for fast arb checking
    let prices: Arc<RwLock<std::collections::HashMap<String, PriceUpdate>>> =
        Arc::new(RwLock::new(std::collections::HashMap::new()));

    // Spawn periodic stats printer
    let stats_clone = stats.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let s = stats_clone.read().await;
            info!(
                "Stats: {} opportunities | {} trades ({} successful) | ${} profit",
                s.opportunities_found,
                s.trades_executed,
                s.trades_successful,
                s.total_profit
            );
        }
    });

    // Spawn market refresh task
    let scanner_clone = scanner.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300)); // Every 5 minutes
        loop {
            interval.tick().await;
            if let Err(e) = scanner_clone.refresh_markets().await {
                warn!("Failed to refresh markets: {}", e);
            }
        }
    });

    // Reconnection loop with exponential backoff
    let mut reconnect_delay = Duration::from_secs(1);
    let max_reconnect_delay = Duration::from_secs(60);

    loop {
        let token_ids = scanner.get_watched_token_ids();
        info!("Subscribing to {} token feeds", token_ids.len());

        match WsClient::connect(token_ids).await {
            Ok(mut ws_client) => {
                // Reset reconnect delay on successful connection
                reconnect_delay = Duration::from_secs(1);

                // Process WebSocket updates
                while let Some(update) = ws_client.rx.recv().await {
                    // Update price cache
                    {
                        let mut price_map = prices.write().await;
                        price_map.insert(update.asset_id.clone(), update.clone());
                    }

                    // Quick check for arbitrage using cached prices
                    if let Some(opp) = check_arb_from_cache(&prices, &scanner, &config).await {
                        let mut stats_guard = stats.write().await;
                        stats_guard.opportunities_found += 1;
                        handle_opportunity(&opp, scanner.clone(), &mut stats_guard, &config).await;
                    }
                }

                warn!("WebSocket connection closed, reconnecting in {:?}...", reconnect_delay);
            }
            Err(e) => {
                error!("Failed to connect to WebSocket: {}. Retrying in {:?}...", e, reconnect_delay);
            }
        }

        // Wait before reconnecting
        tokio::time::sleep(reconnect_delay).await;

        // Exponential backoff (double the delay, up to max)
        reconnect_delay = (reconnect_delay * 2).min(max_reconnect_delay);
    }
}

/// Check for arbitrage using cached WebSocket prices
async fn check_arb_from_cache(
    prices: &Arc<RwLock<std::collections::HashMap<String, PriceUpdate>>>,
    scanner: &ArbitrageScanner,
    config: &Config,
) -> Option<ArbitrageOpportunity> {
    // This is a simplified check - the full implementation would
    // map token pairs and check combined prices from the cache
    // For now, trigger a full scan when we see price updates

    // To avoid excessive scanning, we rely on the scanner's batch approach
    // A production system would maintain a proper price index
    None
}

/// Handle a detected arbitrage opportunity
async fn handle_opportunity(
    opp: &ArbitrageOpportunity,
    scanner: Arc<ArbitrageScanner>,
    stats: &mut BotStats,
    config: &Config,
) {
    info!("===========================================");
    info!("  ARBITRAGE OPPORTUNITY DETECTED!");
    info!("===========================================");
    info!("  Market: {}", opp.market_id);
    info!("  YES ask: ${}", opp.yes_ask_price);
    info!("  NO ask:  ${}", opp.no_ask_price);
    info!("  Combined: ${}", opp.combined_price);
    info!("  Profit/share: ${}", opp.profit_per_share);
    info!("  Max size: {}", opp.max_size);
    info!("  Expected profit: ${}", opp.expected_profit(config.order_size));
    info!("===========================================");

    if config.dry_run {
        info!("DRY RUN - Skipping trade execution");
        return;
    }

    // Determine order size (minimum of config size and available liquidity)
    let size = config.order_size.min(opp.max_size);

    if size < Decimal::ONE {
        warn!("Order size too small, skipping");
        return;
    }

    // Execute the arbitrage
    stats.trades_executed += 1;

    match scanner.client().execute_arbitrage(opp, size).await {
        Ok((yes_resp, no_resp)) => {
            if yes_resp.success && no_resp.success {
                stats.trades_successful += 1;
                stats.total_profit += opp.expected_profit(size);
                info!("Trade successful! Locked profit: ${}", opp.expected_profit(size));
            } else {
                warn!(
                    "Trade partially failed - YES: {}, NO: {}",
                    yes_resp.success, no_resp.success
                );
                if let Some(err) = yes_resp.error_msg {
                    warn!("YES error: {}", err);
                }
                if let Some(err) = no_resp.error_msg {
                    warn!("NO error: {}", err);
                }
            }
        }
        Err(e) => {
            error!("Trade execution failed: {}", e);
        }
    }
}
