//! Prediction tracking and accuracy engine for OpenFang.
//!
//! Tracks market predictions, evaluates them against actual outcomes,
//! and calculates accuracy metrics over time.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::debug;

/// Prediction tracking engine with SQLite backend.
#[derive(Clone)]
pub struct PredictionTracker {
    conn: Arc<Mutex<Connection>>,
}

/// A prediction record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prediction {
    pub id: i64,
    pub ticker: String,
    pub direction: String, // "up", "down", "flat"
    pub entry_price: f64,
    pub target_price: f64,
    pub confidence: f64, // 0-100
    pub timeframe_days: i64,
    pub reasoning: Option<String>,
    pub strategy: Option<String>,
    pub status: String, // "open", "correct", "incorrect", "expired"
    pub actual_price: Option<f64>,
    pub accuracy_score: Option<f64>,
    pub created_at: String,
    pub evaluated_at: Option<String>,
}

impl PredictionTracker {
    /// Open or create a prediction tracking database.
    pub fn open(db_path: &PathBuf) -> Result<Self, String> {
        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open prediction tracker DB: {e}"))?;

        let tracker = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        tracker.init_schema()?;
        Ok(tracker)
    }

    /// Initialize database schema.
    fn init_schema(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS predictions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ticker TEXT NOT NULL,
                direction TEXT NOT NULL CHECK(direction IN ('up', 'down', 'flat')),
                entry_price REAL NOT NULL,
                target_price REAL NOT NULL,
                confidence REAL NOT NULL CHECK(confidence >= 0 AND confidence <= 100),
                timeframe_days INTEGER NOT NULL,
                reasoning TEXT,
                strategy TEXT,
                status TEXT DEFAULT 'open' CHECK(status IN ('open', 'correct', 'incorrect', 'expired')),
                actual_price REAL,
                accuracy_score REAL,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP,
                evaluated_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_predictions_ticker ON predictions(ticker);
            CREATE INDEX IF NOT EXISTS idx_predictions_status ON predictions(status);
            CREATE INDEX IF NOT EXISTS idx_predictions_created ON predictions(created_at);
            CREATE INDEX IF NOT EXISTS idx_predictions_strategy ON predictions(strategy);
            "
        )
        .map_err(|e| format!("Failed to initialize prediction tracker schema: {e}"))?;

        Ok(())
    }

    /// Log a new prediction.
    #[allow(clippy::too_many_arguments)]
    pub fn log_prediction(
        &self,
        ticker: &str,
        direction: &str,
        target_price: f64,
        confidence: f64,
        timeframe_days: i64,
        reasoning: Option<String>,
        entry_price: f64,
        strategy: Option<String>,
    ) -> Result<i64, String> {
        // Validate inputs
        if !["up", "down", "flat"].contains(&direction) {
            return Err(format!("Invalid direction: {direction}. Must be 'up', 'down', or 'flat'"));
        }
        if !(0.0..=100.0).contains(&confidence) {
            return Err(format!("Invalid confidence: {confidence}. Must be between 0 and 100"));
        }
        if timeframe_days <= 0 {
            return Err("Timeframe must be positive".to_string());
        }

        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let timestamp = chrono::Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO predictions (ticker, direction, entry_price, target_price, confidence, timeframe_days, reasoning, strategy, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![ticker, direction, entry_price, target_price, confidence, timeframe_days, reasoning, strategy, timestamp],
        )
        .map_err(|e| format!("Failed to log prediction: {e}"))?;

        let id = conn.last_insert_rowid();
        debug!(ticker, direction, confidence, "Prediction logged with ID {}", id);

        Ok(id)
    }

    /// Evaluate open predictions against current prices.
    pub fn evaluate_predictions(
        &self,
        ticker: Option<&str>,
        current_prices: &std::collections::HashMap<String, f64>,
    ) -> Result<String, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let query = if let Some(t) = ticker {
            format!("SELECT id, ticker, direction, entry_price, target_price, timeframe_days, created_at FROM predictions WHERE status = 'open' AND ticker = '{}'", t)
        } else {
            "SELECT id, ticker, direction, entry_price, target_price, timeframe_days, created_at FROM predictions WHERE status = 'open'".to_string()
        };

        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let predictions = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
            ))
        }).map_err(|e| e.to_string())?;

        let mut evaluated = 0;
        let mut correct = 0;
        let mut incorrect = 0;
        let mut expired = 0;

        for pred in predictions {
            let (id, ticker, direction, entry_price, target_price, timeframe_days, created_at) = pred.map_err(|e| e.to_string())?;

            // Check if prediction has expired
            let created = chrono::DateTime::parse_from_rfc3339(&created_at)
                .map_err(|e| format!("Invalid timestamp: {e}"))?;
            let now = chrono::Utc::now();
            let elapsed_days = (now.signed_duration_since(created)).num_days();

            if elapsed_days > timeframe_days {
                // Expired without hitting target
                conn.execute(
                    "UPDATE predictions SET status = 'expired', evaluated_at = ? WHERE id = ?",
                    rusqlite::params![now.to_rfc3339(), id],
                ).map_err(|e| e.to_string())?;
                expired += 1;
                continue;
            }

            // Get current price
            if let Some(&current_price) = current_prices.get(&ticker) {
                let target_move = target_price - entry_price;
                let actual_move = current_price - entry_price;
                let move_pct = if target_move != 0.0 { actual_move / target_move } else { 0.0 };

                let (status, accuracy_score) = match direction.as_str() {
                    "up" => {
                        if actual_move >= target_move * 0.5 {
                            ("correct", (move_pct.min(1.0) * 100.0).max(50.0))
                        } else if actual_move < 0.0 {
                            ("incorrect", 0.0)
                        } else {
                            continue; // Still in progress
                        }
                    }
                    "down" => {
                        if actual_move <= target_move * 0.5 {
                            ("correct", (move_pct.abs().min(1.0) * 100.0).max(50.0))
                        } else if actual_move > 0.0 {
                            ("incorrect", 0.0)
                        } else {
                            continue; // Still in progress
                        }
                    }
                    "flat" => {
                        let pct_change = (actual_move / entry_price).abs();
                        if pct_change < 0.02 {
                            ("correct", (100.0 - pct_change * 5000.0).max(50.0))
                        } else {
                            ("incorrect", 0.0)
                        }
                    }
                    _ => continue,
                };

                conn.execute(
                    "UPDATE predictions SET status = ?, actual_price = ?, accuracy_score = ?, evaluated_at = ? WHERE id = ?",
                    rusqlite::params![status, current_price, accuracy_score, now.to_rfc3339(), id],
                ).map_err(|e| e.to_string())?;

                evaluated += 1;
                if status == "correct" {
                    correct += 1;
                } else {
                    incorrect += 1;
                }
            }
        }

        Ok(format!(
            "Evaluated {} predictions: {} correct, {} incorrect, {} expired",
            evaluated, correct, incorrect, expired
        ))
    }

    /// Calculate accuracy statistics over a time period.
    pub fn calculate_accuracy(&self, period_days: Option<i64>) -> Result<String, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let cutoff = if let Some(days) = period_days {
            let cutoff_time = chrono::Utc::now() - chrono::Duration::days(days);
            cutoff_time.to_rfc3339()
        } else {
            "1970-01-01T00:00:00Z".to_string() // All time
        };

        // Overall accuracy
        let mut stmt = conn.prepare(
            "SELECT COUNT(*), SUM(CASE WHEN status = 'correct' THEN 1 ELSE 0 END)
             FROM predictions
             WHERE status IN ('correct', 'incorrect') AND created_at >= ?"
        ).map_err(|e| e.to_string())?;

        let (total, correct): (i64, i64) = stmt.query_row([&cutoff], |row| {
            Ok((row.get(0)?, row.get(1)?))
        }).map_err(|e| e.to_string())?;

        let overall_accuracy = if total > 0 {
            (correct as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        // Per-ticker accuracy
        let mut stmt = conn.prepare(
            "SELECT ticker, COUNT(*), SUM(CASE WHEN status = 'correct' THEN 1 ELSE 0 END)
             FROM predictions
             WHERE status IN ('correct', 'incorrect') AND created_at >= ?
             GROUP BY ticker
             ORDER BY COUNT(*) DESC
             LIMIT 10"
        ).map_err(|e| e.to_string())?;

        let ticker_stats = stmt.query_map([&cutoff], |row| {
            let ticker: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let correct: i64 = row.get(2)?;
            let accuracy = (correct as f64 / count as f64) * 100.0;
            Ok(format!("  {}: {:.1}% ({}/{})", ticker, accuracy, correct, count))
        }).map_err(|e| e.to_string())?;

        let mut ticker_lines = Vec::new();
        for stat in ticker_stats {
            ticker_lines.push(stat.map_err(|e| e.to_string())?);
        }

        // Per-strategy accuracy
        let mut stmt = conn.prepare(
            "SELECT strategy, COUNT(*), SUM(CASE WHEN status = 'correct' THEN 1 ELSE 0 END)
             FROM predictions
             WHERE status IN ('correct', 'incorrect') AND created_at >= ? AND strategy IS NOT NULL
             GROUP BY strategy
             ORDER BY COUNT(*) DESC"
        ).map_err(|e| e.to_string())?;

        let strategy_stats = stmt.query_map([&cutoff], |row| {
            let strategy: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let correct: i64 = row.get(2)?;
            let accuracy = (correct as f64 / count as f64) * 100.0;
            Ok(format!("  {}: {:.1}% ({}/{})", strategy, accuracy, correct, count))
        }).map_err(|e| e.to_string())?;

        let mut strategy_lines = Vec::new();
        for stat in strategy_stats {
            strategy_lines.push(stat.map_err(|e| e.to_string())?);
        }

        let period_label = if let Some(days) = period_days {
            format!("{}-day", days)
        } else {
            "all-time".to_string()
        };

        let mut result = format!(
            "Prediction Accuracy ({}):\n\nOverall: {:.1}% ({}/{})\n",
            period_label, overall_accuracy, correct, total
        );

        if !ticker_lines.is_empty() {
            result.push_str("\nBy Ticker:\n");
            result.push_str(&ticker_lines.join("\n"));
        }

        if !strategy_lines.is_empty() {
            result.push_str("\n\nBy Strategy:\n");
            result.push_str(&strategy_lines.join("\n"));
        }

        Ok(result)
    }

    /// List predictions with optional filtering.
    pub fn list_predictions(&self, status: Option<&str>, limit: Option<i64>) -> Result<Vec<Prediction>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let query = if let Some(s) = status {
            format!(
                "SELECT id, ticker, direction, entry_price, target_price, confidence, timeframe_days, reasoning, strategy, status, actual_price, accuracy_score, created_at, evaluated_at
                 FROM predictions
                 WHERE status = '{}'
                 ORDER BY created_at DESC
                 LIMIT {}",
                s,
                limit.unwrap_or(50)
            )
        } else {
            format!(
                "SELECT id, ticker, direction, entry_price, target_price, confidence, timeframe_days, reasoning, strategy, status, actual_price, accuracy_score, created_at, evaluated_at
                 FROM predictions
                 ORDER BY created_at DESC
                 LIMIT {}",
                limit.unwrap_or(50)
            )
        };

        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let predictions = stmt.query_map([], |row| {
            Ok(Prediction {
                id: row.get(0)?,
                ticker: row.get(1)?,
                direction: row.get(2)?,
                entry_price: row.get(3)?,
                target_price: row.get(4)?,
                confidence: row.get(5)?,
                timeframe_days: row.get(6)?,
                reasoning: row.get(7)?,
                strategy: row.get(8)?,
                status: row.get(9)?,
                actual_price: row.get(10)?,
                accuracy_score: row.get(11)?,
                created_at: row.get(12)?,
                evaluated_at: row.get(13)?,
            })
        }).map_err(|e| e.to_string())?;

        let mut result = Vec::new();
        for pred in predictions {
            result.push(pred.map_err(|e| e.to_string())?);
        }

        Ok(result)
    }

    /// Get structured accuracy statistics for a time period.
    pub fn get_accuracy_stats(&self, period_days: Option<i64>) -> Result<serde_json::Value, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let cutoff = if let Some(days) = period_days {
            let cutoff_time = chrono::Utc::now() - chrono::Duration::days(days);
            cutoff_time.to_rfc3339()
        } else {
            "1970-01-01T00:00:00Z".to_string()
        };

        let (total, correct): (i64, i64) = conn.query_row(
            "SELECT COUNT(*), SUM(CASE WHEN status = 'correct' THEN 1 ELSE 0 END)
             FROM predictions
             WHERE status IN ('correct', 'incorrect') AND created_at >= ?",
            [&cutoff],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).map_err(|e| e.to_string())?;

        let accuracy_pct = if total > 0 {
            (correct as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        Ok(serde_json::json!({
            "period_days": period_days,
            "total_predictions": total,
            "correct_predictions": correct,
            "accuracy_pct": accuracy_pct,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn create_test_tracker() -> PredictionTracker {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique_id = format!("{}_{}", chrono::Utc::now().timestamp_millis(), COUNTER.fetch_add(1, Ordering::Relaxed));
        let db_path = std::env::temp_dir().join(format!("test_predictions_{}.db", unique_id));
        PredictionTracker::open(&db_path).unwrap()
    }

    #[test]
    fn test_log_prediction() {
        let tracker = create_test_tracker();

        let id = tracker.log_prediction(
            "AAPL",
            "up",
            175.0,
            85.0,
            30,
            Some("Strong technical indicators".to_string()),
            150.0,
            Some("technical".to_string()),
        ).unwrap();

        assert!(id > 0);
    }

    #[test]
    fn test_log_prediction_invalid_direction() {
        let tracker = create_test_tracker();

        let result = tracker.log_prediction(
            "AAPL",
            "sideways",
            175.0,
            85.0,
            30,
            None,
            150.0,
            None,
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid direction"));
    }

    #[test]
    fn test_log_prediction_invalid_confidence() {
        let tracker = create_test_tracker();

        let result = tracker.log_prediction(
            "AAPL",
            "up",
            175.0,
            150.0,
            30,
            None,
            150.0,
            None,
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid confidence"));
    }

    #[test]
    fn test_evaluate_predictions() {
        let tracker = create_test_tracker();

        // Log a prediction
        tracker.log_prediction(
            "AAPL",
            "up",
            175.0,
            85.0,
            30,
            None,
            150.0,
            None,
        ).unwrap();

        // Evaluate with current price showing the prediction was correct
        let mut prices = HashMap::new();
        prices.insert("AAPL".to_string(), 165.0);

        let result = tracker.evaluate_predictions(None, &prices).unwrap();
        assert!(result.contains("correct"));
    }

    #[test]
    fn test_calculate_accuracy() {
        let tracker = create_test_tracker();

        // Log and evaluate some predictions
        tracker.log_prediction("AAPL", "up", 175.0, 85.0, 30, None, 150.0, Some("technical".to_string())).unwrap();
        tracker.log_prediction("TSLA", "down", 200.0, 75.0, 30, None, 250.0, Some("fundamental".to_string())).unwrap();

        let mut prices = HashMap::new();
        prices.insert("AAPL".to_string(), 165.0);
        prices.insert("TSLA".to_string(), 220.0);

        tracker.evaluate_predictions(None, &prices).unwrap();

        let result = tracker.calculate_accuracy(None).unwrap();
        assert!(result.contains("Prediction Accuracy"));
        assert!(result.contains("Overall"));
    }

    #[test]
    fn test_list_predictions() {
        let tracker = create_test_tracker();

        tracker.log_prediction("AAPL", "up", 175.0, 85.0, 30, None, 150.0, None).unwrap();
        tracker.log_prediction("TSLA", "down", 200.0, 75.0, 30, None, 250.0, None).unwrap();

        let predictions = tracker.list_predictions(Some("open"), Some(10)).unwrap();
        assert_eq!(predictions.len(), 2);
        assert_eq!(predictions[0].ticker, "TSLA"); // Most recent first
        assert_eq!(predictions[1].ticker, "AAPL");
    }
}

