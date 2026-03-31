//! Embedding-based smart tool selection for OpenFang.
//!
//! Uses semantic similarity between user messages and tool descriptions to select
//! the most relevant tools for each request, reducing token overhead from ~3k-5k to ~500-2k.

use crate::embedding::cosine_similarity;
use openfang_types::tool::ToolDefinition;
use reqwest;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Configuration for tool selector.
#[derive(Debug, Clone)]
pub struct ToolSelectorConfig {
    /// Embedding model to use (e.g., "nomic-embed-text").
    pub embedding_model: String,
    /// Number of top tools to select per request.
    pub top_k: usize,
    /// Tools to always include, regardless of similarity score.
    pub always_include: Vec<String>,
    /// Whether tool selection is enabled.
    pub enabled: bool,
}

impl Default for ToolSelectorConfig {
    fn default() -> Self {
        Self {
            embedding_model: "nomic-embed-text".to_string(),
            top_k: 10,
            always_include: vec![
                "web_search".to_string(),
                "memory_store".to_string(),
                "memory_query".to_string(),
                "shell_exec".to_string(),
            ],
            enabled: true,
        }
    }
}

/// Ollama-specific embedding request (different from OpenAI-compatible endpoint).
#[derive(Serialize)]
struct OllamaEmbedRequest {
    model: String,
    input: Vec<String>,
}

/// Ollama embedding response.
#[derive(Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

/// Tool selector using embedding-based similarity matching.
pub struct ToolSelector {
    /// Pre-computed tool embeddings: (tool_name, description, embedding).
    tool_embeddings: Vec<(String, String, Vec<f32>)>,
    /// Tools to always include.
    always_include: Vec<String>,
    /// Number of top tools to select.
    top_k: usize,
    /// Embedding model name.
    embedding_model: String,
    /// HTTP client for Ollama API.
    client: reqwest::Client,
}

impl ToolSelector {
    /// Create a new tool selector and pre-compute embeddings for all tools.
    ///
    /// This should be called once at startup with all available tools.
    /// The embeddings are cached and reused for all subsequent selections.
    pub async fn new(
        tools: &[ToolDefinition],
        config: ToolSelectorConfig,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if tools.is_empty() {
            warn!("ToolSelector initialized with zero tools");
            return Ok(Self {
                tool_embeddings: vec![],
                always_include: config.always_include,
                top_k: config.top_k,
                embedding_model: config.embedding_model,
                client: reqwest::Client::new(),
            });
        }

        info!(
            count = tools.len(),
            model = %config.embedding_model,
            "Initializing tool selector with embedding model"
        );

        // Build searchable text for each tool: "{name}: {description}"
        let descriptions: Vec<String> = tools
            .iter()
            .map(|t| {
                // Extract parameter names from input_schema for better matching
                let params = extract_param_names(&t.input_schema);
                if params.is_empty() {
                    format!("{}: {}", t.name, t.description)
                } else {
                    format!("{}: {}. Parameters: {}", t.name, t.description, params.join(", "))
                }
            })
            .collect();

        // Batch embed all tool descriptions via Ollama API
        let client = reqwest::Client::new();
        let resp = client
            .post("http://localhost:11434/api/embed")
            .json(&OllamaEmbedRequest {
                model: config.embedding_model.clone(),
                input: descriptions.clone(),
            })
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "Ollama embedding API returned status {}: {}",
                status, error_text
            )
            .into());
        }

        let embed_resp: OllamaEmbedResponse = resp.json().await?;

        if embed_resp.embeddings.len() != tools.len() {
            return Err(format!(
                "Embedding count mismatch: expected {}, got {}",
                tools.len(),
                embed_resp.embeddings.len()
            )
            .into());
        }

        // Zip tools with their embeddings
        let tool_embeddings: Vec<(String, String, Vec<f32>)> = tools
            .iter()
            .zip(descriptions)
            .zip(embed_resp.embeddings)
            .map(|(( tool, desc), emb)| (tool.name.clone(), desc, emb))
            .collect();

        info!(
            tools = tool_embeddings.len(),
            dims = tool_embeddings.first().map(|(_, _, e)| e.len()).unwrap_or(0),
            "Tool embeddings computed successfully"
        );

        Ok(Self {
            tool_embeddings,
            always_include: config.always_include,
            top_k: config.top_k,
            embedding_model: config.embedding_model,
            client,
        })
    }

    /// Select the most relevant tools for a given user message.
    ///
    /// Returns the top_k most similar tools plus all always_include tools.
    pub async fn select_tools(
        &self,
        message: &str,
        all_tools: &[ToolDefinition],
    ) -> Result<Vec<ToolDefinition>, Box<dyn std::error::Error + Send + Sync>> {
        if self.tool_embeddings.is_empty() || message.is_empty() {
            // Fallback to all tools if selector is not initialized
            return Ok(all_tools.to_vec());
        }

        // Embed the user message
        let resp = self
            .client
            .post("http://localhost:11434/api/embed")
            .json(&OllamaEmbedRequest {
                model: self.embedding_model.clone(),
                input: vec![message.to_string()],
            })
            .send()
            .await?;

        let embed_resp: OllamaEmbedResponse = resp.json().await?;
        let msg_embedding = &embed_resp.embeddings[0];

        // Compute cosine similarity with all tools
        let mut scores: Vec<(String, f32)> = self
            .tool_embeddings
            .iter()
            .map(|(name, _, tool_emb)| {
                let sim = cosine_similarity(msg_embedding, tool_emb);
                (name.clone(), sim)
            })
            .collect();

        // Sort by similarity (descending)
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Build selected tool names set
        let mut selected_names: Vec<String> = self.always_include.clone();

        // Add top_k similar tools
        for (name, score) in scores.iter().take(self.top_k) {
            if !selected_names.contains(name) {
                selected_names.push(name.clone());
            }
            debug!(tool = %name, similarity = %score, "Tool selected by similarity");
        }

        // Filter all_tools to only include selected ones
        let selected_tools: Vec<ToolDefinition> = all_tools
            .iter()
            .filter(|tool| selected_names.contains(&tool.name))
            .cloned()
            .collect();

        info!(
            message_preview = &message[..message.len().min(50)],
            selected = selected_tools.len(),
            total = all_tools.len(),
            "Embedding-based tool selection complete"
        );

        Ok(selected_tools)
    }
}

/// Extract parameter names from a JSON schema object.
fn extract_param_names(schema: &serde_json::Value) -> Vec<String> {
    if let Some(obj) = schema.as_object() {
        if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
            return props.keys().cloned().collect();
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_param_names() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "ticker": {"type": "string"},
                "quantity": {"type": "number"}
            }
        });
        let params = extract_param_names(&schema);
        assert_eq!(params.len(), 2);
        assert!(params.contains(&"ticker".to_string()));
        assert!(params.contains(&"quantity".to_string()));
    }

    #[test]
    fn test_extract_param_names_empty() {
        let schema = serde_json::json!({});
        let params = extract_param_names(&schema);
        assert!(params.is_empty());
    }
}

