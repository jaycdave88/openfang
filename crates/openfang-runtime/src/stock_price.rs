//! Real-time stock price fetching from Google Finance.
//!
//! Provides HTTP-based price scraping with 5-minute caching to avoid rate limits.
//! No API key required — uses public Google Finance pages.

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

/// Fetch real-time stock price for a single ticker.
///
/// Returns the latest price from Google Finance or an error if not found.
/// Results are cached for 5 minutes to avoid hammering the endpoint.
///
/// # Example
/// ```no_run
/// # tokio_test::block_on(async {
/// let price = openfang_runtime::stock_price::fetch_stock_price("AAPL").await?;
/// println!("AAPL: ${:.2}", price);
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

    // Try each exchange until we find a price
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
}

