//! Real-time price fetching for stocks (Google Finance) and crypto (CoinGecko).
//!
//! Provides HTTP-based price scraping with 5-minute caching to avoid rate limits.
//! No API key required — uses public Google Finance pages and CoinGecko free API.
//! Automatically detects crypto tickers (BTC, ETH, SOL, etc.) vs stock tickers.

use dashmap::DashMap;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

/// Price cache entry with timestamp.
#[derive(Clone, Debug)]
struct CachedPrice {
    price: f64,
    fetched_at: SystemTime,
}

/// Global price cache with 5-minute TTL.
static PRICE_CACHE: LazyLock<DashMap<String, CachedPrice>> = LazyLock::new(DashMap::new);

/// Cache TTL: 5 minutes.
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Exchanges to try in order.
const EXCHANGES: &[&str] = &["NASDAQ", "NYSE", "AMEX"];

// ---------------------------------------------------------------------------
// Crypto ticker → CoinGecko ID mapping
// ---------------------------------------------------------------------------

/// Known crypto ticker → CoinGecko API ID mappings.
const CRYPTO_TICKERS: &[(&str, &str)] = &[
    ("BTC", "bitcoin"),
    ("ETH", "ethereum"),
    ("SOL", "solana"),
    ("ADA", "cardano"),
    ("DOT", "polkadot"),
    ("AVAX", "avalanche-2"),
    ("MATIC", "matic-network"),
    ("LINK", "chainlink"),
    ("ATOM", "cosmos"),
    ("UNI", "uniswap"),
    ("XRP", "ripple"),
    ("DOGE", "dogecoin"),
    ("SHIB", "shiba-inu"),
    ("LTC", "litecoin"),
    ("BCH", "bitcoin-cash"),
    ("NEAR", "near"),
    ("APT", "aptos"),
    ("ARB", "arbitrum"),
    ("OP", "optimism"),
    ("SUI", "sui"),
    ("SEI", "sei-network"),
    ("PEPE", "pepe"),
    ("FET", "fetch-ai"),
    ("RENDER", "render-token"),
    ("INJ", "injective-protocol"),
    ("TIA", "celestia"),
    ("ALGO", "algorand"),
    ("FIL", "filecoin"),
    ("ICP", "internet-computer"),
    ("HBAR", "hedera-hashgraph"),
];

/// Check if a ticker is a known cryptocurrency.
pub fn is_crypto_ticker(ticker: &str) -> bool {
    let upper = ticker.to_uppercase();
    CRYPTO_TICKERS.iter().any(|(t, _)| *t == upper)
}

/// Get the CoinGecko API ID for a crypto ticker.
fn crypto_to_coingecko_id(ticker: &str) -> Option<&'static str> {
    let upper = ticker.to_uppercase();
    CRYPTO_TICKERS.iter().find(|(t, _)| *t == upper).map(|(_, id)| *id)
}

/// Fetch real-time price for a single ticker (stock or crypto).
///
/// Automatically detects crypto tickers (BTC, ETH, SOL, etc.) and routes
/// to CoinGecko. All other tickers go to Google Finance.
/// Results are cached for 5 minutes.
///
/// # Example
/// ```no_run
/// # tokio_test::block_on(async {
/// let price = openfang_runtime::stock_price::fetch_stock_price("AAPL").await?;
/// println!("AAPL: ${:.2}", price);
/// let btc = openfang_runtime::stock_price::fetch_stock_price("BTC").await?;
/// println!("BTC: ${:.2}", btc);
/// # Ok::<(), String>(())
/// # });
/// ```
pub async fn fetch_stock_price(ticker: &str) -> Result<f64, String> {
    let ticker_upper = ticker.to_uppercase();

    // Check cache first
    if let Some(cached) = PRICE_CACHE.get(&ticker_upper) {
        if cached.fetched_at.elapsed().unwrap_or(CACHE_TTL) < CACHE_TTL {
            return Ok(cached.price);
        }
    }

    // Route to CoinGecko for crypto tickers
    if let Some(coingecko_id) = crypto_to_coingecko_id(&ticker_upper) {
        return fetch_crypto_price(&ticker_upper, coingecko_id).await;
    }

    // Try each exchange until we find a price (stocks)
    for exchange in EXCHANGES {
        let url = format!("https://www.google.com/finance/quote/{}:{}", ticker_upper, exchange);

        match fetch_price_from_url(&url).await {
            Ok(price) => {
                // Cache the result
                PRICE_CACHE.insert(ticker_upper.clone(), CachedPrice {
                    price,
                    fetched_at: SystemTime::now(),
                });
                return Ok(price);
            }
            Err(_) => continue, // Try next exchange
        }
    }

    Err(format!("Could not fetch price for {} from any exchange (tried: {})", ticker, EXCHANGES.join(", ")))
}

/// Fetch prices for multiple tickers in parallel.
///
/// Returns a HashMap with ticker → price mappings. Tickers that fail to fetch
/// are omitted from the result (check the HashMap size to detect failures).
///
/// # Example
/// ```no_run
/// # tokio_test::block_on(async {
/// let prices = openfang_runtime::stock_price::fetch_stock_prices(&["AAPL", "MSFT", "NVDA"]).await?;
/// for (ticker, price) in &prices {
///     println!("{}: ${:.2}", ticker, price);
/// }
/// # Ok::<(), String>(())
/// # });
/// ```
pub async fn fetch_stock_prices(tickers: &[&str]) -> Result<std::collections::HashMap<String, f64>, String> {
    use futures::future::join_all;

    let futures: Vec<_> = tickers.iter().map(|ticker| async move {
        match fetch_stock_price(ticker).await {
            Ok(price) => Some((ticker.to_uppercase(), price)),
            Err(_) => None,
        }
    }).collect();

    let results = join_all(futures).await;
    let prices: std::collections::HashMap<String, f64> = results.into_iter().flatten().collect();

    if prices.is_empty() {
        Err("Failed to fetch any prices".to_string())
    } else {
        Ok(prices)
    }
}

// ---------------------------------------------------------------------------
// CoinGecko crypto price fetching
// ---------------------------------------------------------------------------

/// Fetch crypto price from CoinGecko free API.
///
/// Uses the simple/price endpoint which is free and requires no API key.
/// Rate limit: ~10-30 calls/minute on free tier.
async fn fetch_crypto_price(ticker: &str, coingecko_id: &str) -> Result<f64, String> {
    let url = format!(
        "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies=usd",
        coingecko_id
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(crate::USER_AGENT)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("CoinGecko request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("CoinGecko HTTP {}", response.status()));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse CoinGecko response: {e}"))?;

    let price = body[coingecko_id]["usd"]
        .as_f64()
        .ok_or_else(|| format!("CoinGecko returned no USD price for {}", ticker))?;

    // Cache the result
    PRICE_CACHE.insert(ticker.to_string(), CachedPrice {
        price,
        fetched_at: SystemTime::now(),
    });

    Ok(price)
}

/// Fetch crypto prices for multiple tickers in a single CoinGecko call.
///
/// More efficient than calling `fetch_crypto_price` individually because
/// CoinGecko supports comma-separated IDs in one request.
pub async fn fetch_crypto_prices(tickers: &[&str]) -> Result<std::collections::HashMap<String, f64>, String> {
    let id_pairs: Vec<(&str, &str)> = tickers
        .iter()
        .filter_map(|t| {
            let upper = t.to_uppercase();
            CRYPTO_TICKERS.iter()
                .find(|(ct, _)| *ct == upper)
                .map(|(ct, id)| (*ct, *id))
        })
        .collect();

    if id_pairs.is_empty() {
        return Err("No valid crypto tickers provided".to_string());
    }

    let ids: Vec<&str> = id_pairs.iter().map(|(_, id)| *id).collect();
    let url = format!(
        "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies=usd",
        ids.join(",")
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(crate::USER_AGENT)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("CoinGecko request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("CoinGecko HTTP {}", response.status()));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse CoinGecko response: {e}"))?;

    let mut prices = std::collections::HashMap::new();
    for (ticker, coingecko_id) in &id_pairs {
        if let Some(price) = body[*coingecko_id]["usd"].as_f64() {
            prices.insert(ticker.to_string(), price);
            PRICE_CACHE.insert(ticker.to_string(), CachedPrice {
                price,
                fetched_at: SystemTime::now(),
            });
        }
    }

    if prices.is_empty() {
        Err("Failed to fetch any crypto prices".to_string())
    } else {
        Ok(prices)
    }
}

// ---------------------------------------------------------------------------
// Google Finance stock price fetching
// ---------------------------------------------------------------------------

/// HTTP GET and parse price from Google Finance HTML.
async fn fetch_price_from_url(url: &str) -> Result<f64, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(crate::USER_AGENT)
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let html = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?;

    // Parse: data-last-price="123.45"
    parse_price_from_html(&html)
}

/// Extract price from HTML using regex.
fn parse_price_from_html(html: &str) -> Result<f64, String> {
    // Look for: data-last-price="123.45"
    let pattern = r#"data-last-price="([0-9]+\.?[0-9]*)""#;
    let re = regex_lite::Regex::new(pattern).map_err(|e| format!("Regex error: {e}"))?;

    if let Some(caps) = re.captures(html) {
        if let Some(price_str) = caps.get(1) {
            return price_str.as_str()
                .parse::<f64>()
                .map_err(|e| format!("Failed to parse price: {e}"));
        }
    }

    Err("Price not found in HTML".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_price_from_html() {
        let html = r#"<div data-last-price="248.80" data-currency="USD">AAPL</div>"#;
        let price = parse_price_from_html(html).unwrap();
        assert_eq!(price, 248.80);

        let html2 = r#"<span data-last-price="167.52">NVDA</span>"#;
        let price2 = parse_price_from_html(html2).unwrap();
        assert_eq!(price2, 167.52);
    }

    #[test]
    fn test_parse_price_from_html_not_found() {
        let html = r#"<div>No price here</div>"#;
        let result = parse_price_from_html(html);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Price not found"));
    }

    #[test]
    fn test_price_cache() {
        // Clear cache first
        PRICE_CACHE.clear();

        // Insert a test price
        let ticker = "TEST";
        PRICE_CACHE.insert(ticker.to_string(), CachedPrice {
            price: 100.0,
            fetched_at: SystemTime::now(),
        });

        // Verify it's cached
        assert!(PRICE_CACHE.contains_key(ticker));
        let cached = PRICE_CACHE.get(ticker).unwrap();
        assert_eq!(cached.price, 100.0);

        // Verify cache expiry works
        let old_cache = CachedPrice {
            price: 50.0,
            fetched_at: SystemTime::now() - Duration::from_secs(400), // Older than TTL
        };
        PRICE_CACHE.insert("OLD".to_string(), old_cache);

        let cached_old = PRICE_CACHE.get("OLD").unwrap();
        assert!(cached_old.fetched_at.elapsed().unwrap() > CACHE_TTL);
    }

    #[test]
    fn test_exchange_detection() {
        // Verify we have the right exchanges
        assert_eq!(EXCHANGES.len(), 3);
        assert_eq!(EXCHANGES[0], "NASDAQ");
        assert_eq!(EXCHANGES[1], "NYSE");
        assert_eq!(EXCHANGES[2], "AMEX");
    }

    #[test]
    fn test_is_crypto_ticker() {
        assert!(is_crypto_ticker("BTC"));
        assert!(is_crypto_ticker("btc"));
        assert!(is_crypto_ticker("ETH"));
        assert!(is_crypto_ticker("SOL"));
        assert!(is_crypto_ticker("DOGE"));
        assert!(is_crypto_ticker("XRP"));
        assert!(!is_crypto_ticker("AAPL"));
        assert!(!is_crypto_ticker("MSFT"));
        assert!(!is_crypto_ticker("SPY"));
    }

    #[test]
    fn test_crypto_to_coingecko_id() {
        assert_eq!(crypto_to_coingecko_id("BTC"), Some("bitcoin"));
        assert_eq!(crypto_to_coingecko_id("ETH"), Some("ethereum"));
        assert_eq!(crypto_to_coingecko_id("SOL"), Some("solana"));
        assert_eq!(crypto_to_coingecko_id("DOGE"), Some("dogecoin"));
        assert_eq!(crypto_to_coingecko_id("AAPL"), None);
        assert_eq!(crypto_to_coingecko_id("MSFT"), None);
    }
}

