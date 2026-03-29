//! Self-learning feedback loop for prediction accuracy analysis.
//!
//! Analyzes prediction accuracy over time, identifies patterns, and generates
//! actionable insights to improve trading strategies.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::debug;

/// Self-learning engine with SQLite backend.
#[derive(Clone)]
pub struct SelfLearning {
    conn: Arc<Mutex<Connection>>,
}

/// A weekly report record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeeklyReport {
    pub id: i64,
    pub period_start: String,
    pub period_end: String,
    pub total_predictions: i64,
    pub correct_predictions: i64,
    pub accuracy_pct: f64,
    pub best_strategy: Option<String>,
    pub worst_strategy: Option<String>,
    pub key_insights: Vec<String>,
    pub adjustments_made: Vec<String>,
    pub created_at: String,
}

/// A strategy adjustment record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyAdjustment {
    pub id: i64,
    pub strategy: String,
    pub adjustment: String,
    pub reason: Option<String>,
    pub before_accuracy: Option<f64>,
    pub created_at: String,
}

/// A learning insight record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Learning {
    pub id: i64,
    pub category: String, // 'pattern', 'bias', 'strength', 'weakness'
    pub insight: String,
    pub confidence: Option<f64>,
    pub source_report_id: Option<i64>,
    pub created_at: String,
}

impl SelfLearning {
    /// Open or create a self-learning database.
    pub fn open(db_path: &PathBuf) -> Result<Self, String> {
        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open self-learning DB: {e}"))?;

        let learning = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        learning.init_schema()?;
        Ok(learning)
    }

    /// Initialize database schema.
    fn init_schema(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS weekly_reports (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                period_start TEXT NOT NULL,
                period_end TEXT NOT NULL,
                total_predictions INTEGER NOT NULL,
                correct_predictions INTEGER NOT NULL,
                accuracy_pct REAL NOT NULL,
                best_strategy TEXT,
                worst_strategy TEXT,
                key_insights TEXT NOT NULL,
                adjustments_made TEXT NOT NULL,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS strategy_adjustments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                strategy TEXT NOT NULL,
                adjustment TEXT NOT NULL,
                reason TEXT,
                before_accuracy REAL,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS learnings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                category TEXT NOT NULL CHECK(category IN ('pattern', 'bias', 'strength', 'weakness')),
                insight TEXT NOT NULL,
                confidence REAL,
                source_report_id INTEGER,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_weekly_reports_period ON weekly_reports(period_start, period_end);
            CREATE INDEX IF NOT EXISTS idx_strategy_adjustments_strategy ON strategy_adjustments(strategy);
            CREATE INDEX IF NOT EXISTS idx_learnings_category ON learnings(category);
            "
        )
        .map_err(|e| format!("Failed to initialize self-learning schema: {e}"))?;

        Ok(())
    }

    /// Generate a weekly report analyzing prediction accuracy.
    pub fn generate_report(
        &self,
        period_days: i64,
        predictions: &[crate::prediction_tracker::Prediction],
    ) -> Result<i64, String> {
        let now = chrono::Utc::now();
        let period_start = (now - chrono::Duration::days(period_days)).to_rfc3339();
        let period_end = now.to_rfc3339();

        // Filter predictions within the period
        let period_predictions: Vec<_> = predictions
            .iter()
            .filter(|p| p.created_at >= period_start && p.created_at <= period_end)
            .collect();

        let total_predictions = period_predictions.len() as i64;
        let correct_predictions = period_predictions
            .iter()
            .filter(|p| p.status == "correct")
            .count() as i64;

        let accuracy_pct = if total_predictions > 0 {
            (correct_predictions as f64 / total_predictions as f64) * 100.0
        } else {
            0.0
        };

        // Analyze by strategy
        let mut strategy_stats: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();
        for pred in &period_predictions {
            if let Some(strategy) = &pred.strategy {
                let entry = strategy_stats.entry(strategy.clone()).or_insert((0, 0));
                entry.0 += 1; // total
                if pred.status == "correct" {
                    entry.1 += 1; // correct
                }
            }
        }

        let (best_strategy, worst_strategy) = if !strategy_stats.is_empty() {
            let mut best = ("".to_string(), 0.0);
            let mut worst = ("".to_string(), 100.0);

            for (strategy, (total, correct)) in &strategy_stats {
                if *total > 0 {
                    let acc = (*correct as f64 / *total as f64) * 100.0;
                    if acc > best.1 {
                        best = (strategy.clone(), acc);
                    }
                    if acc < worst.1 {
                        worst = (strategy.clone(), acc);
                    }
                }
            }

            (Some(best.0), Some(worst.0))
        } else {
            (None, None)
        };

        // Generate insights
        let mut key_insights = Vec::new();

        // Insight: Overall accuracy trend
        if accuracy_pct >= 70.0 {
            key_insights.push(format!("Strong overall accuracy at {:.1}%", accuracy_pct));
        } else if accuracy_pct >= 50.0 {
            key_insights.push(format!("Moderate accuracy at {:.1}% - room for improvement", accuracy_pct));
        } else {
            key_insights.push(format!("Low accuracy at {:.1}% - significant adjustments needed", accuracy_pct));
        }

        // Insight: Strategy performance
        if let Some(best) = &best_strategy {
            if let Some((total, correct)) = strategy_stats.get(best) {
                let acc = (*correct as f64 / *total as f64) * 100.0;
                key_insights.push(format!("Best strategy: {} ({:.1}% accuracy)", best, acc));
            }
        }

        if let Some(worst) = &worst_strategy {
            if let Some((total, correct)) = strategy_stats.get(worst) {
                let acc = (*correct as f64 / *total as f64) * 100.0;
                key_insights.push(format!("Worst strategy: {} ({:.1}% accuracy) - consider revising", worst, acc));
            }
        }

        // Insight: Direction bias
        let up_count = period_predictions.iter().filter(|p| p.direction == "up").count();
        let down_count = period_predictions.iter().filter(|p| p.direction == "down").count();
        if up_count > down_count * 2 {
            key_insights.push("Bullish bias detected - consider balancing predictions".to_string());
        } else if down_count > up_count * 2 {
            key_insights.push("Bearish bias detected - consider balancing predictions".to_string());
        }

        // Store the report
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let timestamp = chrono::Utc::now().to_rfc3339();

        let key_insights_json = serde_json::to_string(&key_insights)
            .map_err(|e| format!("Failed to serialize insights: {e}"))?;
        let adjustments_json = serde_json::to_string(&Vec::<String>::new())
            .map_err(|e| format!("Failed to serialize adjustments: {e}"))?;

        conn.execute(
            "INSERT INTO weekly_reports (period_start, period_end, total_predictions, correct_predictions, accuracy_pct, best_strategy, worst_strategy, key_insights, adjustments_made, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                period_start,
                period_end,
                total_predictions,
                correct_predictions,
                accuracy_pct,
                best_strategy,
                worst_strategy,
                key_insights_json,
                adjustments_json,
                timestamp
            ],
        )
        .map_err(|e| format!("Failed to store weekly report: {e}"))?;

        let id = conn.last_insert_rowid();
        debug!(period_days, total_predictions, accuracy_pct, "Weekly report generated with ID {}", id);

        Ok(id)
    }

    /// Get insights by category or strategy.
    pub fn get_insights(&self, strategy: Option<&str>) -> Result<Vec<Learning>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let query = if let Some(s) = strategy {
            format!(
                "SELECT id, category, insight, confidence, source_report_id, created_at
                 FROM learnings
                 WHERE insight LIKE '%{}%'
                 ORDER BY created_at DESC
                 LIMIT 50",
                s
            )
        } else {
            "SELECT id, category, insight, confidence, source_report_id, created_at
             FROM learnings
             ORDER BY created_at DESC
             LIMIT 50"
                .to_string()
        };

        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let learnings = stmt
            .query_map([], |row| {
                Ok(Learning {
                    id: row.get(0)?,
                    category: row.get(1)?,
                    insight: row.get(2)?,
                    confidence: row.get(3)?,
                    source_report_id: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut result = Vec::new();
        for learning in learnings {
            result.push(learning.map_err(|e| e.to_string())?);
        }

        Ok(result)
    }

    /// Record a strategy adjustment.
    pub fn update_strategy(
        &self,
        strategy: &str,
        adjustment: &str,
        reason: Option<String>,
        before_accuracy: Option<f64>,
    ) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let timestamp = chrono::Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO strategy_adjustments (strategy, adjustment, reason, before_accuracy, created_at)
             VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![strategy, adjustment, reason, before_accuracy, timestamp],
        )
        .map_err(|e| format!("Failed to record strategy adjustment: {e}"))?;

        let id = conn.last_insert_rowid();
        debug!(strategy, adjustment, "Strategy adjustment recorded with ID {}", id);

        Ok(id)
    }

    /// Get the most recent weekly report.
    pub fn get_latest_report(&self) -> Result<Option<WeeklyReport>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare(
                "SELECT id, period_start, period_end, total_predictions, correct_predictions, accuracy_pct, best_strategy, worst_strategy, key_insights, adjustments_made, created_at
                 FROM weekly_reports
                 ORDER BY created_at DESC
                 LIMIT 1",
            )
            .map_err(|e| e.to_string())?;

        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;

        if let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let key_insights_json: String = row.get(8).map_err(|e| e.to_string())?;
            let adjustments_json: String = row.get(9).map_err(|e| e.to_string())?;

            let key_insights: Vec<String> = serde_json::from_str(&key_insights_json)
                .map_err(|e| format!("Failed to deserialize insights: {e}"))?;
            let adjustments_made: Vec<String> = serde_json::from_str(&adjustments_json)
                .map_err(|e| format!("Failed to deserialize adjustments: {e}"))?;

            Ok(Some(WeeklyReport {
                id: row.get(0).map_err(|e| e.to_string())?,
                period_start: row.get(1).map_err(|e| e.to_string())?,
                period_end: row.get(2).map_err(|e| e.to_string())?,
                total_predictions: row.get(3).map_err(|e| e.to_string())?,
                correct_predictions: row.get(4).map_err(|e| e.to_string())?,
                accuracy_pct: row.get(5).map_err(|e| e.to_string())?,
                best_strategy: row.get(6).map_err(|e| e.to_string())?,
                worst_strategy: row.get(7).map_err(|e| e.to_string())?,
                key_insights,
                adjustments_made,
                created_at: row.get(10).map_err(|e| e.to_string())?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Get recent learnings with a limit.
    pub fn get_recent_learnings(&self, limit: i64) -> Result<Vec<Learning>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare(
                "SELECT id, category, insight, confidence, source_report_id, created_at
                 FROM learnings
                 ORDER BY created_at DESC
                 LIMIT ?"
            )
            .map_err(|e| e.to_string())?;

        let learnings = stmt
            .query_map([limit], |row| {
                Ok(Learning {
                    id: row.get(0)?,
                    category: row.get(1)?,
                    insight: row.get(2)?,
                    confidence: row.get(3)?,
                    source_report_id: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut result = Vec::new();
        for learning in learnings {
            result.push(learning.map_err(|e| e.to_string())?);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_learning() -> SelfLearning {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique_id = format!(
            "{}_{}",
            chrono::Utc::now().timestamp_millis(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let db_path = std::env::temp_dir().join(format!("test_learning_{}.db", unique_id));
        SelfLearning::open(&db_path).unwrap()
    }

    fn create_test_prediction(
        ticker: &str,
        direction: &str,
        status: &str,
        strategy: Option<String>,
    ) -> crate::prediction_tracker::Prediction {
        crate::prediction_tracker::Prediction {
            id: 1,
            ticker: ticker.to_string(),
            direction: direction.to_string(),
            entry_price: 100.0,
            target_price: 110.0,
            confidence: 75.0,
            timeframe_days: 30,
            reasoning: None,
            strategy,
            status: status.to_string(),
            actual_price: Some(105.0),
            accuracy_score: Some(80.0),
            created_at: chrono::Utc::now().to_rfc3339(),
            evaluated_at: Some(chrono::Utc::now().to_rfc3339()),
        }
    }

    #[test]
    fn test_generate_report() {
        let learning = create_test_learning();

        let predictions = vec![
            create_test_prediction("AAPL", "up", "correct", Some("technical".to_string())),
            create_test_prediction("TSLA", "down", "incorrect", Some("fundamental".to_string())),
            create_test_prediction("MSFT", "up", "correct", Some("technical".to_string())),
        ];

        let id = learning.generate_report(7, &predictions).unwrap();
        assert!(id > 0);

        // Verify the report was stored
        let report = learning.get_latest_report().unwrap();
        assert!(report.is_some());
        let report = report.unwrap();
        assert_eq!(report.total_predictions, 3);
        assert_eq!(report.correct_predictions, 2);
        assert!((report.accuracy_pct - 66.67).abs() < 0.1);
    }

    #[test]
    fn test_update_strategy() {
        let learning = create_test_learning();

        let id = learning
            .update_strategy(
                "technical",
                "Increase confidence threshold to 80%",
                Some("Low accuracy on medium-confidence predictions".to_string()),
                Some(65.0),
            )
            .unwrap();

        assert!(id > 0);
    }

    #[test]
    fn test_get_insights() {
        let learning = create_test_learning();

        // Get insights (should be empty initially)
        let insights = learning.get_insights(None).unwrap();
        assert_eq!(insights.len(), 0);
    }
}
