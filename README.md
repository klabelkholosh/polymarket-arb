# polymarket-arb

A high-performance arbitrage scanner for [Polymarket](https://polymarket.com) prediction markets, written in Rust.

## What it does

Scans Polymarket's binary (YES/NO) markets looking for arbitrage opportunities where:

```
YES_ask_price + NO_ask_price < $1.00
```

When this condition exists, buying both outcomes guarantees a profit since one will pay out $1.00.

## Reality check

**True arbitrage opportunities on Polymarket are extremely rare.** The market is efficient, with professional market makers running 24/7. In testing, combined ask prices typically sum to $1.98-$2.00. This bot is primarily educational—don't expect frequent opportunities.

## Features

- Uses the official [Polymarket CLOB SDK](https://github.com/Polymarket/rs-clob-client)
- Parallel order book fetching for speed
- Parallel order execution with `tokio::join!`
- WebSocket support for real-time price updates
- Dry run mode for safe testing
- Configurable thresholds and order sizes

## Installation

```bash
# Clone the repo
git clone https://github.com/yourusername/polymarket-arb
cd polymarket-arb

# Build (requires Rust 1.70+)
cargo build --release
```

## Configuration

Create a `.env` file:

```bash
# Required: Ethereum private key (without 0x prefix)
# Wallet needs USDC on Polygon for trading
POLYMARKET_PRIVATE_KEY=your_private_key_here

# Arbitrage thresholds
MAX_COMBINED_PRICE=0.99      # Trigger when YES+NO < this
MIN_PROFIT_THRESHOLD=0.01    # Minimum profit per share

# Trading settings
ORDER_SIZE=10.0              # USDC per trade
DRY_RUN=true                 # Set false for live trading

# Scanning settings
POLL_INTERVAL_MS=2000        # Polling frequency
MAX_MARKETS=50               # Markets to monitor
USE_WEBSOCKET=true           # Real-time updates
CRYPTO_ONLY=false            # Filter to crypto markets only

# Logging
RUST_LOG=info
```

## Usage

```bash
# Dry run mode (recommended to start)
cargo run

# With debug logging
RUST_LOG=polymarket_arb=debug cargo run

# Polling mode (no WebSocket)
USE_WEBSOCKET=false cargo run
```

## How it works

1. Authenticates with Polymarket using the official SDK
2. Fetches active markets via `sampling_markets` endpoint
3. Extracts YES/NO token pairs from binary markets
4. Fetches order books in parallel for all monitored tokens
5. Checks if `best_yes_ask + best_no_ask < threshold`
6. If opportunity found and not in dry run, executes both orders in parallel

## Project structure

```
src/
├── main.rs        # Entry point, polling/websocket modes
├── api/
│   ├── client.rs  # Polymarket CLOB client wrapper
│   └── types.rs   # Data structures
├── scanner.rs     # Arbitrage detection logic
├── config.rs      # Environment configuration
└── websocket.rs   # Real-time price feeds
```

## Dependencies

- `tokio` - Async runtime
- `polymarket-client-sdk` - Official Polymarket SDK
- `alloy` - Ethereum signing
- `rust_decimal` - Precise decimal arithmetic
- `dashmap` - Concurrent market cache

## Disclaimer

This software is for educational purposes. Trading on prediction markets involves risk. The authors are not responsible for any financial losses. Always test with `DRY_RUN=true` first.

## License

MIT
