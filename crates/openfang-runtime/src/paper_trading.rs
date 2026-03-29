//! Paper trading engine for OpenFang.
//!
//! Provides a virtual portfolio with SQLite persistence for tracking paper trades,
//! positions, and P&L without risking real capital.

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
    pub shares: i64,
    pub price: f64,
    pub timestamp: String,
    pub reason: Option<String>,
}

/// A portfolio position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub ticker: String,
    pub shares: i64,
    pub avg_cost: f64,
    pub current_value: f64,
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
                shares INTEGER NOT NULL,
                avg_cost REAL NOT NULL,
                current_value REAL NOT NULL
            );
            
            CREATE TABLE IF NOT EXISTS trades (
                id TEXT PRIMARY KEY,
                ticker TEXT NOT NULL,
                side TEXT NOT NULL,
                shares INTEGER NOT NULL,
                price REAL NOT NULL,
                timestamp TEXT NOT NULL,
                reason TEXT
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
            .prepare("SELECT ticker, shares, avg_cost, current_value FROM portfolio")
            .map_err(|e| format!("Failed to prepare query: {e}"))?;
        
        let positions = stmt
            .query_map([], |row| {
                Ok(Position {
                    ticker: row.get(0)?,
                    shares: row.get(1)?,
                    avg_cost: row.get(2)?,
                    current_value: row.get(3)?,
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

    /// Execute a buy order.
    pub fn buy(&self, ticker: &str, shares: i64, price: Option<f64>, reason: Option<String>) -> Result<String, String> {
        if shares <= 0 {
            return Err("Shares must be positive".to_string());
        }

        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        // Get current market price (if not provided, use a placeholder)
        let actual_price = price.unwrap_or(100.0); // In real impl, fetch from market data
        let cost = actual_price * shares as f64;

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

        conn.execute(
            "INSERT INTO trades (id, ticker, side, shares, price, timestamp, reason) VALUES (?, ?, 'buy', ?, ?, ?, ?)",
            rusqlite::params![trade_id, ticker, shares, actual_price, timestamp, reason],
        )
        .map_err(|e| format!("Failed to record trade: {e}"))?;

        // Update or create position
        let existing: Option<(i64, f64)> = conn
            .query_row(
                "SELECT shares, avg_cost FROM portfolio WHERE ticker = ?",
                rusqlite::params![ticker],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        if let Some((existing_shares, existing_avg_cost)) = existing {
            let new_shares = existing_shares + shares;
            let new_avg_cost = ((existing_shares as f64 * existing_avg_cost) + cost) / new_shares as f64;
            let new_value = new_shares as f64 * actual_price;

            conn.execute(
                "UPDATE portfolio SET shares = ?, avg_cost = ?, current_value = ? WHERE ticker = ?",
                rusqlite::params![new_shares, new_avg_cost, new_value, ticker],
            )
            .map_err(|e| format!("Failed to update position: {e}"))?;
        } else {
            conn.execute(
                "INSERT INTO portfolio (ticker, shares, avg_cost, current_value) VALUES (?, ?, ?, ?)",
                rusqlite::params![ticker, shares, actual_price, cost],
            )
            .map_err(|e| format!("Failed to create position: {e}"))?;
        }

        // Update account balance
        let new_balance = balance - cost;
        let new_total_value = total_value; // Total value stays same (cash -> stock)

        conn.execute(
            "UPDATE account SET balance = ?, total_value = ? WHERE id = 1",
            rusqlite::params![new_balance, new_total_value],
        )
        .map_err(|e| format!("Failed to update account: {e}"))?;

        debug!(ticker, shares, price = actual_price, "Paper trade buy executed");

        Ok(format!(
            "Bought {} shares of {} at ${:.2} (total: ${:.2}). Trade ID: {}",
            shares, ticker, actual_price, cost, trade_id
        ))
    }

    /// Execute a sell order.
    pub fn sell(&self, ticker: &str, shares: i64, price: Option<f64>, reason: Option<String>) -> Result<String, String> {
        if shares <= 0 {
            return Err("Shares must be positive".to_string());
        }

        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        // Check if we have the position
        let (existing_shares, avg_cost): (i64, f64) = conn
            .query_row(
                "SELECT shares, avg_cost FROM portfolio WHERE ticker = ?",
                rusqlite::params![ticker],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| format!("No position in {}", ticker))?;

        if shares > existing_shares {
            return Err(format!(
                "Insufficient shares: trying to sell {}, only have {}",
                shares, existing_shares
            ));
        }

        // Get current market price
        let actual_price = price.unwrap_or(100.0); // In real impl, fetch from market data
        let proceeds = actual_price * shares as f64;

        // Record trade
        let trade_id = uuid::Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO trades (id, ticker, side, shares, price, timestamp, reason) VALUES (?, ?, 'sell', ?, ?, ?, ?)",
            rusqlite::params![trade_id, ticker, shares, actual_price, timestamp, reason],
        )
        .map_err(|e| format!("Failed to record trade: {e}"))?;

        // Update position
        let new_shares = existing_shares - shares;
        if new_shares == 0 {
            // Close position
            conn.execute(
                "DELETE FROM portfolio WHERE ticker = ?",
                rusqlite::params![ticker],
            )
            .map_err(|e| format!("Failed to close position: {e}"))?;
        } else {
            // Reduce position
            let new_value = new_shares as f64 * actual_price;
            conn.execute(
                "UPDATE portfolio SET shares = ?, current_value = ? WHERE ticker = ?",
                rusqlite::params![new_shares, new_value, ticker],
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
        let new_total_value = total_value; // Total value stays same (stock -> cash)

        conn.execute(
            "UPDATE account SET balance = ?, total_value = ? WHERE id = 1",
            rusqlite::params![new_balance, new_total_value],
        )
        .map_err(|e| format!("Failed to update account: {e}"))?;

        // Calculate P&L
        let cost_basis = avg_cost * shares as f64;
        let pnl = proceeds - cost_basis;
        let pnl_pct = (pnl / cost_basis) * 100.0;

        debug!(ticker, shares, price = actual_price, pnl, "Paper trade sell executed");

        Ok(format!(
            "Sold {} shares of {} at ${:.2} (total: ${:.2}). P&L: ${:.2} ({:.2}%). Trade ID: {}",
            shares, ticker, actual_price, proceeds, pnl, pnl_pct, trade_id
        ))
    }

    /// Get trade history.
    pub fn get_trade_history(&self, ticker: Option<&str>, limit: Option<usize>) -> Result<Vec<Trade>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let (query, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match ticker {
            Some(t) => (
                "SELECT id, ticker, side, shares, price, timestamp, reason FROM trades WHERE ticker = ? ORDER BY timestamp DESC LIMIT ?".to_string(),
                vec![Box::new(t.to_string()), Box::new(limit.unwrap_or(100) as i64)],
            ),
            None => (
                "SELECT id, ticker, side, shares, price, timestamp, reason FROM trades ORDER BY timestamp DESC LIMIT ?".to_string(),
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
                    shares: row.get(3)?,
                    price: row.get(4)?,
                    timestamp: row.get(5)?,
                    reason: row.get(6)?,
                })
            })
            .map_err(|e| format!("Failed to query trades: {e}"))?
            .collect::<SqliteResult<Vec<_>>>()
            .map_err(|e| format!("Failed to collect trades: {e}"))?;

        Ok(trades)
    }
}

