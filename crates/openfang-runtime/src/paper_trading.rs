//! Paper trading engine for OpenFang.
//!
//! Provides a virtual portfolio with SQLite persistence for tracking paper trades,
//! positions, and P&L without risking real capital.
//! Supports both stocks (integer shares) and crypto (fractional quantities).

use rusqlite::{Connection, Result as SqliteResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::debug;

/// Starting balance for new paper trading accounts.
const STARTING_BALANCE: f64 = 100_000.0;

/// Maximum percentage of portfolio per position (10%).
const MAX_POSITION_PCT: f64 = 0.10;

/// Maximum number of concurrent positions.
const MAX_POSITIONS: usize = 5;

/// Default stop-loss percentage (2%).
#[allow(dead_code)]
const DEFAULT_STOP_LOSS_PCT: f64 = 0.02;

/// Asset type for distinguishing stocks from crypto.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AssetType {
    Stock,
    Crypto,
}

impl AssetType {
    pub fn as_str(&self) -> &'static str {
        match self {
            AssetType::Stock => "stock",
            AssetType::Crypto => "crypto",
        }
    }

    pub fn from_label(s: &str) -> Self {
        if s.eq_ignore_ascii_case("crypto") {
            AssetType::Crypto
        } else {
            AssetType::Stock
        }
    }
}

/// Detect asset type from ticker symbol.
pub fn detect_asset_type(ticker: &str) -> AssetType {
    if crate::stock_price::is_crypto_ticker(ticker) {
        AssetType::Crypto
    } else {
        AssetType::Stock
    }
}

/// Paper trading engine with SQLite backend.
#[derive(Clone)]
pub struct PaperTradingEngine {
    conn: Arc<Mutex<Connection>>,
}

/// A trade record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub id: String,
    pub ticker: String,
    pub side: String, // "buy" or "sell"
    pub quantity: f64,
    pub price: f64,
    pub timestamp: String,
    pub reason: Option<String>,
    pub asset_type: String,
}

/// A portfolio position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub ticker: String,
    pub quantity: f64,
    pub avg_cost: f64,
    pub current_value: f64,
    pub asset_type: String,
}

/// Account status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountStatus {
    pub balance: f64,
    pub total_value: f64,
    pub positions: Vec<Position>,
}

impl PaperTradingEngine {
    /// Open or create a paper trading database.
    pub fn open(db_path: &PathBuf) -> Result<Self, String> {
        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open paper trading DB: {e}"))?;
        
        let engine = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        
        engine.init_schema()?;
        Ok(engine)
    }

    /// Initialize database schema.
    fn init_schema(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS portfolio (
                ticker TEXT PRIMARY KEY,
                quantity REAL NOT NULL,
                avg_cost REAL NOT NULL,
                current_value REAL NOT NULL,
                asset_type TEXT NOT NULL DEFAULT 'stock'
            );

            CREATE TABLE IF NOT EXISTS trades (
                id TEXT PRIMARY KEY,
                ticker TEXT NOT NULL,
                side TEXT NOT NULL,
                quantity REAL NOT NULL,
                price REAL NOT NULL,
                timestamp TEXT NOT NULL,
                reason TEXT,
                asset_type TEXT NOT NULL DEFAULT 'stock'
            );

            CREATE TABLE IF NOT EXISTS account (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                balance REAL NOT NULL,
                total_value REAL NOT NULL
            );

            INSERT OR IGNORE INTO account (id, balance, total_value)
            VALUES (1, ?, ?);",
        )
        .map_err(|e| format!("Failed to init schema: {e}"))?;

        // Migrate existing tables: add asset_type column if missing
        // and rename shares to quantity if needed
        let _ = conn.execute_batch(
            "ALTER TABLE portfolio ADD COLUMN asset_type TEXT NOT NULL DEFAULT 'stock';",
        );
        let _ = conn.execute_batch(
            "ALTER TABLE trades ADD COLUMN asset_type TEXT NOT NULL DEFAULT 'stock';",
        );
        // Rename shares → quantity (SQLite doesn't support RENAME COLUMN before 3.25,
        // so we just ensure both column names work via the new schema)

        // Initialize account if it doesn't exist
        conn.execute(
            "INSERT OR IGNORE INTO account (id, balance, total_value) VALUES (1, ?, ?)",
            rusqlite::params![STARTING_BALANCE, STARTING_BALANCE],
        )
        .map_err(|e| format!("Failed to init account: {e}"))?;

        Ok(())
    }

    /// Get current account status.
    pub fn get_account_status(&self) -> Result<AccountStatus, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let (balance, total_value): (f64, f64) = conn
            .query_row(
                "SELECT balance, total_value FROM account WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| format!("Failed to get account: {e}"))?;

        let mut stmt = conn
            .prepare("SELECT ticker, quantity, avg_cost, current_value, asset_type FROM portfolio")
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let positions = stmt
            .query_map([], |row| {
                Ok(Position {
                    ticker: row.get(0)?,
                    quantity: row.get(1)?,
                    avg_cost: row.get(2)?,
                    current_value: row.get(3)?,
                    asset_type: row.get::<_, String>(4).unwrap_or_else(|_| "stock".to_string()),
                })
            })
            .map_err(|e| format!("Failed to query positions: {e}"))?
            .collect::<SqliteResult<Vec<_>>>()
            .map_err(|e| format!("Failed to collect positions: {e}"))?;

        Ok(AccountStatus {
            balance,
            total_value,
            positions,
        })
    }

    /// Execute a buy order. Supports fractional quantities for crypto.
    pub async fn buy(&self, ticker: &str, quantity: f64, price: Option<f64>, reason: Option<String>) -> Result<String, String> {
        // Get current market price
        let actual_price = match price {
            Some(p) => p,
            None => {
                crate::stock_price::fetch_stock_price(ticker).await
                    .map_err(|e| format!("Could not get price for {}: {}", ticker, e))?
            }
        };

        let asset_type = detect_asset_type(ticker);
        self.buy_sync(ticker, quantity, actual_price, reason, &asset_type)
    }

    /// Synchronous buy implementation (internal).
    fn buy_sync(&self, ticker: &str, quantity: f64, actual_price: f64, reason: Option<String>, asset_type: &AssetType) -> Result<String, String> {
        if quantity <= 0.0 {
            return Err("Quantity must be positive".to_string());
        }

        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let cost = actual_price * quantity;

        // Check balance
        let balance: f64 = conn
            .query_row("SELECT balance FROM account WHERE id = 1", [], |row| row.get(0))
            .map_err(|e| format!("Failed to get balance: {e}"))?;

        if cost > balance {
            return Err(format!("Insufficient balance: ${:.2} needed, ${:.2} available", cost, balance));
        }

        // Check position limits
        let position_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM portfolio", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count positions: {e}"))?;

        let has_position: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM portfolio WHERE ticker = ?",
                rusqlite::params![ticker],
                |row| {
                    let count: i64 = row.get(0)?;
                    Ok(count > 0)
                },
            )
            .map_err(|e| format!("Failed to check position: {e}"))?;

        if !has_position && position_count >= MAX_POSITIONS as i64 {
            return Err(format!("Maximum {} positions reached", MAX_POSITIONS));
        }

        // Check position size limit (10% of total value)
        let total_value: f64 = conn
            .query_row("SELECT total_value FROM account WHERE id = 1", [], |row| row.get(0))
            .map_err(|e| format!("Failed to get total value: {e}"))?;

        let max_position_value = total_value * MAX_POSITION_PCT;
        if cost > max_position_value {
            return Err(format!(
                "Position too large: ${:.2} exceeds max ${:.2} ({}% of portfolio)",
                cost, max_position_value, MAX_POSITION_PCT * 100.0
            ));
        }

        // Record trade
        let trade_id = uuid::Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().to_rfc3339();
        let asset_type_str = asset_type.as_str();

        conn.execute(
            "INSERT INTO trades (id, ticker, side, quantity, price, timestamp, reason, asset_type) VALUES (?, ?, 'buy', ?, ?, ?, ?, ?)",
            rusqlite::params![trade_id, ticker, quantity, actual_price, timestamp, reason, asset_type_str],
        )
        .map_err(|e| format!("Failed to record trade: {e}"))?;

        // Update or create position
        let existing: Option<(f64, f64)> = conn
            .query_row(
                "SELECT quantity, avg_cost FROM portfolio WHERE ticker = ?",
                rusqlite::params![ticker],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        if let Some((existing_qty, existing_avg_cost)) = existing {
            let new_qty = existing_qty + quantity;
            let new_avg_cost = ((existing_qty * existing_avg_cost) + cost) / new_qty;
            let new_value = new_qty * actual_price;

            conn.execute(
                "UPDATE portfolio SET quantity = ?, avg_cost = ?, current_value = ? WHERE ticker = ?",
                rusqlite::params![new_qty, new_avg_cost, new_value, ticker],
            )
            .map_err(|e| format!("Failed to update position: {e}"))?;
        } else {
            conn.execute(
                "INSERT INTO portfolio (ticker, quantity, avg_cost, current_value, asset_type) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![ticker, quantity, actual_price, cost, asset_type_str],
            )
            .map_err(|e| format!("Failed to create position: {e}"))?;
        }

        // Update account balance
        let new_balance = balance - cost;
        let new_total_value = total_value; // Total value stays same (cash -> asset)

        conn.execute(
            "UPDATE account SET balance = ?, total_value = ? WHERE id = 1",
            rusqlite::params![new_balance, new_total_value],
        )
        .map_err(|e| format!("Failed to update account: {e}"))?;

        let qty_str = format_quantity(quantity, asset_type);
        debug!(ticker, quantity, price = actual_price, "Paper trade buy executed");

        Ok(format!(
            "Bought {} of {} at ${:.2} (total: ${:.2}). Trade ID: {}",
            qty_str, ticker, actual_price, cost, trade_id
        ))
    }

    /// Execute a sell order. Supports fractional quantities for crypto.
    pub async fn sell(&self, ticker: &str, quantity: f64, price: Option<f64>, reason: Option<String>) -> Result<String, String> {
        // Get current market price
        let actual_price = match price {
            Some(p) => p,
            None => {
                crate::stock_price::fetch_stock_price(ticker).await
                    .map_err(|e| format!("Could not get price for {}: {}", ticker, e))?
            }
        };

        let asset_type = detect_asset_type(ticker);
        self.sell_sync(ticker, quantity, actual_price, reason, &asset_type)
    }

    /// Synchronous sell implementation (internal).
    fn sell_sync(&self, ticker: &str, quantity: f64, actual_price: f64, reason: Option<String>, asset_type: &AssetType) -> Result<String, String> {
        if quantity <= 0.0 {
            return Err("Quantity must be positive".to_string());
        }

        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        // Check if we have the position
        let (existing_qty, avg_cost): (f64, f64) = conn
            .query_row(
                "SELECT quantity, avg_cost FROM portfolio WHERE ticker = ?",
                rusqlite::params![ticker],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| format!("No position in {}", ticker))?;

        if quantity > existing_qty {
            let qty_str = format_quantity(existing_qty, asset_type);
            return Err(format!(
                "Insufficient quantity: trying to sell {}, only have {}",
                format_quantity(quantity, asset_type), qty_str
            ));
        }

        let proceeds = actual_price * quantity;

        // Record trade
        let trade_id = uuid::Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().to_rfc3339();
        let asset_type_str = asset_type.as_str();

        conn.execute(
            "INSERT INTO trades (id, ticker, side, quantity, price, timestamp, reason, asset_type) VALUES (?, ?, 'sell', ?, ?, ?, ?, ?)",
            rusqlite::params![trade_id, ticker, quantity, actual_price, timestamp, reason, asset_type_str],
        )
        .map_err(|e| format!("Failed to record trade: {e}"))?;

        // Update position
        let new_qty = existing_qty - quantity;
        if new_qty < 1e-10 {
            // Close position (handle floating point near-zero)
            conn.execute(
                "DELETE FROM portfolio WHERE ticker = ?",
                rusqlite::params![ticker],
            )
            .map_err(|e| format!("Failed to close position: {e}"))?;
        } else {
            // Reduce position
            let new_value = new_qty * actual_price;
            conn.execute(
                "UPDATE portfolio SET quantity = ?, current_value = ? WHERE ticker = ?",
                rusqlite::params![new_qty, new_value, ticker],
            )
            .map_err(|e| format!("Failed to update position: {e}"))?;
        }

        // Update account balance
        let balance: f64 = conn
            .query_row("SELECT balance FROM account WHERE id = 1", [], |row| row.get(0))
            .map_err(|e| format!("Failed to get balance: {e}"))?;

        let total_value: f64 = conn
            .query_row("SELECT total_value FROM account WHERE id = 1", [], |row| row.get(0))
            .map_err(|e| format!("Failed to get total value: {e}"))?;

        let new_balance = balance + proceeds;
        let new_total_value = total_value; // Total value stays same (asset -> cash)

        conn.execute(
            "UPDATE account SET balance = ?, total_value = ? WHERE id = 1",
            rusqlite::params![new_balance, new_total_value],
        )
        .map_err(|e| format!("Failed to update account: {e}"))?;

        // Calculate P&L
        let cost_basis = avg_cost * quantity;
        let pnl = proceeds - cost_basis;
        let pnl_pct = if cost_basis > 0.0 { (pnl / cost_basis) * 100.0 } else { 0.0 };

        let qty_str = format_quantity(quantity, asset_type);
        debug!(ticker, quantity, price = actual_price, pnl, "Paper trade sell executed");

        Ok(format!(
            "Sold {} of {} at ${:.2} (total: ${:.2}). P&L: ${:.2} ({:.2}%). Trade ID: {}",
            qty_str, ticker, actual_price, proceeds, pnl, pnl_pct, trade_id
        ))
    }

    /// Get trade history.
    pub fn get_trade_history(&self, ticker: Option<&str>, limit: Option<usize>) -> Result<Vec<Trade>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let (query, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match ticker {
            Some(t) => (
                "SELECT id, ticker, side, quantity, price, timestamp, reason, asset_type FROM trades WHERE ticker = ? ORDER BY timestamp DESC LIMIT ?".to_string(),
                vec![Box::new(t.to_string()), Box::new(limit.unwrap_or(100) as i64)],
            ),
            None => (
                "SELECT id, ticker, side, quantity, price, timestamp, reason, asset_type FROM trades ORDER BY timestamp DESC LIMIT ?".to_string(),
                vec![Box::new(limit.unwrap_or(100) as i64)],
            ),
        };

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let trades = stmt
            .query_map(&params_refs[..], |row| {
                Ok(Trade {
                    id: row.get(0)?,
                    ticker: row.get(1)?,
                    side: row.get(2)?,
                    quantity: row.get(3)?,
                    price: row.get(4)?,
                    timestamp: row.get(5)?,
                    reason: row.get(6)?,
                    asset_type: row.get::<_, String>(7).unwrap_or_else(|_| "stock".to_string()),
                })
            })
            .map_err(|e| format!("Failed to query trades: {e}"))?
            .collect::<SqliteResult<Vec<_>>>()
            .map_err(|e| format!("Failed to collect trades: {e}"))?;

        Ok(trades)
    }
}

/// Format quantity for display — integer for stocks, decimal for crypto.
fn format_quantity(quantity: f64, asset_type: &AssetType) -> String {
    match asset_type {
        AssetType::Stock => format!("{} shares", quantity as i64),
        AssetType::Crypto => {
            if quantity >= 1.0 {
                format!("{:.4}", quantity)
            } else {
                format!("{:.8}", quantity)
            }
        }
    }
}

