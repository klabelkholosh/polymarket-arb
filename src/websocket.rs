//! WebSocket client for real-time Polymarket price feeds
//! Subscribes to order book updates for faster arbitrage detection

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::api::{OrderBookEntry, WsEvent};

/// WebSocket endpoint
const WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

/// Subscription message
#[derive(Debug, Serialize)]
struct SubscribeMessage {
    #[serde(rename = "type")]
    msg_type: String,
    assets_ids: Vec<String>,
}

/// Price update event sent through channel
#[derive(Debug, Clone)]
pub struct PriceUpdate {
    pub asset_id: String,
    pub best_bid: Option<rust_decimal::Decimal>,
    pub best_ask: Option<rust_decimal::Decimal>,
}

/// WebSocket connection handler
pub struct WsClient {
    /// Channel to receive price updates
    pub rx: mpsc::Receiver<PriceUpdate>,
    /// Shutdown signal
    shutdown_tx: mpsc::Sender<()>,
}

impl WsClient {
    /// Connect and subscribe to token IDs
    pub async fn connect(token_ids: Vec<String>) -> Result<Self> {
        if token_ids.is_empty() {
            anyhow::bail!("No token IDs to subscribe to");
        }

        info!("Connecting to WebSocket with {} tokens...", token_ids.len());

        let (ws_stream, _) = connect_async(WS_URL)
            .await
            .context("Failed to connect to WebSocket")?;

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to book updates
        let subscribe_msg = SubscribeMessage {
            msg_type: "subscribe".to_string(),
            assets_ids: token_ids.clone(),
        };

        let msg_json = serde_json::to_string(&subscribe_msg)?;
        write.send(Message::Text(msg_json)).await?;
        info!("Subscribed to {} token feeds", token_ids.len());

        // Create channels
        let (tx, rx) = mpsc::channel::<PriceUpdate>(1000);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        // Spawn reader task
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("WebSocket shutting down");
                        break;
                    }
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                if let Err(e) = Self::handle_message(&text, &tx).await {
                                    debug!("Failed to handle message: {}", e);
                                }
                            }
                            Some(Ok(Message::Ping(data))) => {
                                debug!("Received ping");
                                // Pong is handled automatically by tungstenite
                            }
                            Some(Ok(Message::Close(_))) => {
                                info!("WebSocket closed by server");
                                break;
                            }
                            Some(Err(e)) => {
                                error!("WebSocket error: {}", e);
                                break;
                            }
                            None => {
                                info!("WebSocket stream ended");
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        Ok(Self { rx, shutdown_tx })
    }

    /// Handle incoming WebSocket message
    async fn handle_message(text: &str, tx: &mpsc::Sender<PriceUpdate>) -> Result<()> {
        // Try parsing as array of events (Polymarket sends batches)
        if let Ok(events) = serde_json::from_str::<Vec<WsEventWrapper>>(text) {
            for event in events {
                Self::process_event(event, tx).await?;
            }
            return Ok(());
        }

        // Try parsing as single event
        if let Ok(event) = serde_json::from_str::<WsEventWrapper>(text) {
            Self::process_event(event, tx).await?;
        }

        Ok(())
    }

    /// Process a single WebSocket event
    async fn process_event(event: WsEventWrapper, tx: &mpsc::Sender<PriceUpdate>) -> Result<()> {
        match event.event_type.as_str() {
            "book" => {
                let update = PriceUpdate {
                    asset_id: event.asset_id.clone(),
                    best_bid: event.bids.as_ref()
                        .and_then(|b| b.first())
                        .map(|e| e.price),
                    best_ask: event.asks.as_ref()
                        .and_then(|a| a.first())
                        .map(|e| e.price),
                };

                if update.best_ask.is_some() || update.best_bid.is_some() {
                    tx.send(update).await.ok();
                }
            }
            "price_change" | "last_trade_price" => {
                if let Some(price) = event.price {
                    debug!("Price update for {}: {}", event.asset_id, price);
                }
            }
            _ => {
                debug!("Unknown event type: {}", event.event_type);
            }
        }

        Ok(())
    }

    /// Shutdown the WebSocket connection
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(()).await;
    }
}

/// Flexible WebSocket event wrapper for parsing
#[derive(Debug, Deserialize)]
struct WsEventWrapper {
    event_type: String,
    asset_id: String,
    #[serde(default)]
    bids: Option<Vec<OrderBookEntry>>,
    #[serde(default)]
    asks: Option<Vec<OrderBookEntry>>,
    #[serde(default)]
    price: Option<rust_decimal::Decimal>,
    #[serde(default)]
    market: Option<String>,
}
