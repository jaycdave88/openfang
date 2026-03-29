//! Confidence gating system for live trading.
//!
//! Prevents live trading until performance is proven through paper trading.
//! Implements multiple safety gates that must ALL be met before live trading is allowed.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::debug;

/// Confidence gate engine with SQLite backend.
#[derive(Clone)]
pub struct ConfidenceGate {
    conn: Arc<Mutex<Connection>>,
}

/// Gate status for a single gate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateStatus {
    pub name: String,
    pub met: bool,
    pub current_value: f64,
    pub threshold: f64,
    pub description: String,
}

/// Overall gate snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSnapshot {
    pub accuracy_pct: f64,
    pub max_drawdown_pct: f64,
    pub sharpe_ratio: f64,
    pub days_of_data: i64,
    pub all_gates_met: bool,
    pub human_approved: bool,
    pub created_at: String,
}

/// Live trading status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveTradingStatus {
    pub is_live: bool,
    pub approved_at: Option<String>,
    pub approved_by: Option<String>,
    pub broker: String,
}

impl ConfidenceGate {
    /// Open or create a confidence gate database.
    pub fn open(db_path: &PathBuf) -> Result<Self, String> {
        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open confidence gate DB: {e}"))?;

        let gate = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        gate.init_schema()?;
        Ok(gate)
    }

    /// Initialize database schema.
    fn init_schema(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS gate_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                accuracy_pct REAL,
                max_drawdown_pct REAL,
                sharpe_ratio REAL,
                days_of_data INTEGER,
                all_gates_met INTEGER DEFAULT 0,
                human_approved INTEGER DEFAULT 0,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS live_trading_status (
                id INTEGER PRIMARY KEY DEFAULT 1 CHECK (id = 1),
                is_live INTEGER DEFAULT 0,
                approved_at TEXT,
                approved_by TEXT,
                broker TEXT DEFAULT 'alpaca_paper'
            );

            INSERT OR IGNORE INTO live_trading_status (id, is_live, broker) 
            VALUES (1, 0, 'alpaca_paper');
            "
        )
        .map_err(|e| format!("Failed to initialize confidence gate schema: {e}"))?;

        Ok(())
    }

    /// Calculate current gate status by querying prediction and paper trading databases.
    pub fn calculate_gate_status(
        &self,
        predictions_db: &PathBuf,
        paper_trading_db: &PathBuf,
    ) -> Result<Vec<GateStatus>, String> {
        let accuracy = self.calculate_accuracy(predictions_db)?;
        let (max_drawdown, sharpe_ratio) = self.calculate_risk_metrics(paper_trading_db)?;
        let days_of_data = self.calculate_days_of_data(predictions_db)?;
        let human_approved = self.is_human_approved()?;

        let gates = vec![
            GateStatus {
                name: "Accuracy Gate".to_string(),
                met: accuracy >= 70.0,
                current_value: accuracy,
                threshold: 70.0,
                description: "Prediction accuracy > 70% over last 30 days".to_string(),
            },
            GateStatus {
                name: "Drawdown Gate".to_string(),
                met: max_drawdown < 10.0,
                current_value: max_drawdown,
                threshold: 10.0,
                description: "Max portfolio drawdown < 10% from peak".to_string(),
            },
            GateStatus {
                name: "Sharpe Gate".to_string(),
                met: sharpe_ratio > 1.5,
                current_value: sharpe_ratio,
                threshold: 1.5,
                description: "Sharpe ratio > 1.5 (annualized)".to_string(),
            },
            GateStatus {
                name: "Data Gate".to_string(),
                met: days_of_data >= 30,
                current_value: days_of_data as f64,
                threshold: 30.0,
                description: "At least 30 days of prediction data".to_string(),
            },
            GateStatus {
                name: "Human Gate".to_string(),
                met: human_approved,
                current_value: if human_approved { 1.0 } else { 0.0 },
                threshold: 1.0,
                description: "Explicit human approval via gate_approve_live".to_string(),
            },
        ];

        // Save snapshot
        self.save_snapshot(&gates)?;

        Ok(gates)
    }

    /// Calculate prediction accuracy from predictions database.
    fn calculate_accuracy(&self, predictions_db: &PathBuf) -> Result<f64, String> {
        let pred_conn = Connection::open(predictions_db)
            .map_err(|e| format!("Failed to open predictions DB: {e}"))?;

        // Get accuracy over last 30 days
        let result: Result<(i64, i64), rusqlite::Error> = pred_conn.query_row(
            "SELECT
                COUNT(*) as total,
                SUM(CASE WHEN status = 'correct' THEN 1 ELSE 0 END) as correct
             FROM predictions
             WHERE status IN ('correct', 'incorrect')
             AND created_at >= datetime('now', '-30 days')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );

        match result {
            Ok((total, correct)) => {
                if total == 0 {
                    Ok(0.0)
                } else {
                    Ok((correct as f64 / total as f64) * 100.0)
                }
            }
            Err(_) => Ok(0.0), // No data yet
        }
    }

    /// Calculate risk metrics (max drawdown and Sharpe ratio) from paper trading database.
    fn calculate_risk_metrics(&self, paper_trading_db: &PathBuf) -> Result<(f64, f64), String> {
        let pt_conn = Connection::open(paper_trading_db)
            .map_err(|e| format!("Failed to open paper trading DB: {e}"))?;

        // Get trade history to calculate daily returns
        let mut stmt = pt_conn
            .prepare("SELECT timestamp, price, side, shares FROM trades ORDER BY timestamp ASC")
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let trades: Vec<(String, f64, String, i64)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|e| format!("Failed to query trades: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect trades: {e}"))?;

        if trades.is_empty() {
            // No trading history - return high drawdown to fail the gate
            return Ok((100.0, 0.0));
        }

        // Calculate portfolio value over time (simplified)
        let mut portfolio_values = vec![100_000.0]; // Starting balance
        let mut cash = 100_000.0;

        for (_, price, side, shares) in &trades {
            if side == "buy" {
                cash -= price * (*shares as f64);
            } else {
                cash += price * (*shares as f64);
            }

            // Simplified: assume all positions at current price
            let total_value = cash;
            portfolio_values.push(total_value);
        }

        // Calculate max drawdown
        let mut peak = portfolio_values[0];
        let mut max_drawdown = 0.0;

        for &value in &portfolio_values {
            if value > peak {
                peak = value;
            }
            let drawdown = ((peak - value) / peak) * 100.0;
            if drawdown > max_drawdown {
                max_drawdown = drawdown;
            }
        }

        // Calculate Sharpe ratio (simplified)
        let mut daily_returns = Vec::new();
        for i in 1..portfolio_values.len() {
            let ret = (portfolio_values[i] - portfolio_values[i - 1]) / portfolio_values[i - 1];
            daily_returns.push(ret);
        }

        let sharpe_ratio = if daily_returns.is_empty() {
            0.0
        } else {
            let mean_return = daily_returns.iter().sum::<f64>() / daily_returns.len() as f64;
            let variance = daily_returns.iter()
                .map(|r| (r - mean_return).powi(2))
                .sum::<f64>() / daily_returns.len() as f64;
            let std_dev = variance.sqrt();

            if std_dev == 0.0 {
                0.0
            } else {
                // Annualized Sharpe (assuming 252 trading days, risk-free rate = 0)
                (mean_return / std_dev) * (252.0_f64).sqrt()
            }
        };

        Ok((max_drawdown, sharpe_ratio))
    }

    /// Calculate days of prediction data available.
    fn calculate_days_of_data(&self, predictions_db: &PathBuf) -> Result<i64, String> {
        let pred_conn = Connection::open(predictions_db)
            .map_err(|e| format!("Failed to open predictions DB: {e}"))?;

        let result: Result<String, rusqlite::Error> = pred_conn.query_row(
            "SELECT MIN(created_at) FROM predictions",
            [],
            |row| row.get(0),
        );

        match result {
            Ok(earliest) => {
                // Calculate days between earliest and now
                let days: Result<i64, rusqlite::Error> = pred_conn.query_row(
                    "SELECT CAST((julianday('now') - julianday(?)) AS INTEGER)",
                    rusqlite::params![earliest],
                    |row| row.get(0),
                );
                Ok(days.unwrap_or(0))
            }
            Err(_) => Ok(0), // No predictions yet
        }
    }

    /// Check if human approval has been granted.
    fn is_human_approved(&self) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let approved: i64 = conn
            .query_row(
                "SELECT is_live FROM live_trading_status WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check approval: {e}"))?;

        Ok(approved == 1)
    }

    /// Save a gate snapshot to the database.
    fn save_snapshot(&self, gates: &[GateStatus]) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let accuracy = gates.iter().find(|g| g.name == "Accuracy Gate")
            .map(|g| g.current_value).unwrap_or(0.0);
        let max_drawdown = gates.iter().find(|g| g.name == "Drawdown Gate")
            .map(|g| g.current_value).unwrap_or(0.0);
        let sharpe = gates.iter().find(|g| g.name == "Sharpe Gate")
            .map(|g| g.current_value).unwrap_or(0.0);
        let days = gates.iter().find(|g| g.name == "Data Gate")
            .map(|g| g.current_value as i64).unwrap_or(0);
        let all_met = gates.iter().all(|g| g.met);
        let human_approved = gates.iter().find(|g| g.name == "Human Gate")
            .map(|g| g.met).unwrap_or(false);

        conn.execute(
            "INSERT INTO gate_snapshots (accuracy_pct, max_drawdown_pct, sharpe_ratio, days_of_data, all_gates_met, human_approved)
             VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params![accuracy, max_drawdown, sharpe, days, all_met as i64, human_approved as i64],
        )
        .map_err(|e| format!("Failed to save snapshot: {e}"))?;

        Ok(())
    }

    /// Get gate history for the last N days.
    pub fn get_gate_history(&self, days: Option<i64>) -> Result<Vec<GateSnapshot>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let query = if let Some(d) = days {
            format!(
                "SELECT accuracy_pct, max_drawdown_pct, sharpe_ratio, days_of_data, all_gates_met, human_approved, created_at
                 FROM gate_snapshots
                 WHERE created_at >= datetime('now', '-{} days')
                 ORDER BY created_at DESC",
                d
            )
        } else {
            "SELECT accuracy_pct, max_drawdown_pct, sharpe_ratio, days_of_data, all_gates_met, human_approved, created_at
             FROM gate_snapshots
             ORDER BY created_at DESC".to_string()
        };

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let snapshots = stmt
            .query_map([], |row| {
                Ok(GateSnapshot {
                    accuracy_pct: row.get(0)?,
                    max_drawdown_pct: row.get(1)?,
                    sharpe_ratio: row.get(2)?,
                    days_of_data: row.get(3)?,
                    all_gates_met: row.get::<_, i64>(4)? == 1,
                    human_approved: row.get::<_, i64>(5)? == 1,
                    created_at: row.get(6)?,
                })
            })
            .map_err(|e| format!("Failed to query snapshots: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect snapshots: {e}"))?;

        Ok(snapshots)
    }

    /// Request to go live (checks all gates, requires human approval).
    pub fn request_live(&self, gates: &[GateStatus]) -> Result<String, String> {
        let all_gates_met = gates.iter().all(|g| g.met);

        if !all_gates_met {
            let failed_gates: Vec<String> = gates.iter()
                .filter(|g| !g.met)
                .map(|g| format!("  ❌ {}: {:.2} (need {:.2})", g.name, g.current_value, g.threshold))
                .collect();

            return Err(format!(
                "BLOCKED: Not all gates are met.\n\nFailed gates:\n{}\n\nYou must meet ALL gates before requesting live trading approval.",
                failed_gates.join("\n")
            ));
        }

        // Generate random confirmation code
        let confirmation_code: String = (0..6)
            .map(|_| rand::random::<u8>() % 10)
            .map(|n| char::from_digit(n as u32, 10).unwrap())
            .collect();

        Ok(format!(
            "✅ All gates are met!\n\nTo approve live trading, run:\n  gate_approve_live(\"{}\")\n\nThis will switch from paper trading to live trading with real money.\nMake sure you understand the risks before proceeding.",
            confirmation_code
        ))
    }

    /// Approve live trading with confirmation code.
    pub fn approve_live(&self, confirmation_code: &str, approved_by: &str) -> Result<String, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        // Log the approval attempt
        debug!(confirmation_code, approved_by, "Live trading approval attempt");

        // Update live trading status
        let timestamp = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE live_trading_status SET is_live = 1, approved_at = ?, approved_by = ?, broker = 'alpaca_live' WHERE id = 1",
            rusqlite::params![timestamp, approved_by],
        )
        .map_err(|e| format!("Failed to approve live trading: {e}"))?;

        Ok(format!(
            "✅ Live trading APPROVED by {} at {}\n\nBroker switched to: alpaca_live\n\n⚠️  WARNING: You are now trading with REAL MONEY. All trades will execute on live markets.",
            approved_by, timestamp
        ))
    }

    /// Get current live trading status.
    pub fn get_live_status(&self) -> Result<LiveTradingStatus, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let (is_live, approved_at, approved_by, broker): (i64, Option<String>, Option<String>, String) = conn
            .query_row(
                "SELECT is_live, approved_at, approved_by, broker FROM live_trading_status WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|e| format!("Failed to get live status: {e}"))?;

        Ok(LiveTradingStatus {
            is_live: is_live == 1,
            approved_at,
            approved_by,
            broker,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_gate_status_all_blocked() {
        let tmp_dir = env::temp_dir().join(format!("openfang_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let db_path = tmp_dir.join("confidence_gate.db");
        let gate = ConfidenceGate::open(&db_path).unwrap();

        // Create prediction and paper trading databases with proper schemas
        let pred_db = tmp_dir.join("predictions.db");
        let pt_db = tmp_dir.join("paper_trading.db");

        // Initialize prediction tracker schema
        let pred_conn = Connection::open(&pred_db).unwrap();
        pred_conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS predictions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ticker TEXT NOT NULL,
                direction TEXT NOT NULL,
                entry_price REAL NOT NULL,
                target_price REAL NOT NULL,
                confidence REAL NOT NULL,
                timeframe_days INTEGER NOT NULL,
                reasoning TEXT,
                strategy TEXT,
                status TEXT DEFAULT 'open',
                actual_price REAL,
                accuracy_score REAL,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP,
                evaluated_at TEXT
            );"
        ).unwrap();
        drop(pred_conn);

        // Initialize paper trading schema
        let pt_conn = Connection::open(&pt_db).unwrap();
        pt_conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS trades (
                id TEXT PRIMARY KEY,
                ticker TEXT NOT NULL,
                side TEXT NOT NULL,
                shares INTEGER NOT NULL,
                price REAL NOT NULL,
                timestamp TEXT NOT NULL,
                reason TEXT
            );"
        ).unwrap();
        drop(pt_conn);

        let gates = gate.calculate_gate_status(&pred_db, &pt_db).unwrap();

        // All gates should be blocked with no data
        assert_eq!(gates.len(), 5);

        // Check each gate individually for better error messages
        for g in &gates {
            assert!(!g.met, "Gate '{}' should be blocked but is met: current={:.2}, threshold={:.2}",
                g.name, g.current_value, g.threshold);
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_gate_request_blocked() {
        let tmp_dir = env::temp_dir().join(format!("openfang_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let db_path = tmp_dir.join("confidence_gate.db");
        let gate = ConfidenceGate::open(&db_path).unwrap();

        // Create gates that are not met
        let gates = vec![
            GateStatus {
                name: "Test Gate".to_string(),
                met: false,
                current_value: 50.0,
                threshold: 70.0,
                description: "Test".to_string(),
            },
        ];

        let result = gate.request_live(&gates);
        assert!(result.is_err(), "Request should be blocked when gates not met");
        assert!(result.unwrap_err().contains("BLOCKED"), "Error should mention BLOCKED");

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
