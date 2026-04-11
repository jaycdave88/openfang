//! Model routing — auto-selects cheap/mid/expensive models by query complexity.
//!
//! The router scores each `CompletionRequest` based on heuristics (token count,
//! tool availability, code markers, conversation depth) and picks the cheapest
//! model that can handle the task.
//!
//! ## Two-stage dynamic tool selection
//!
//! When `dynamic_tool_selection` is enabled in the routing config, a fast
//! classification model (Stage 1) categorises the user message into tool
//! categories *before* the main LLM call. Only tools matching the selected
//! categories are forwarded (Stage 2), dramatically reducing system-prompt size.

use crate::llm_driver::CompletionRequest;
use openfang_types::agent::ModelRoutingConfig;
use openfang_types::tool::ToolDefinition;
use std::collections::HashMap;
use tracing::warn;

/// Task complexity tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    /// Quick lookup, greetings, simple Q&A — use the cheapest model.
    Simple,
    /// Standard conversational task — use a mid-tier model.
    Medium,
    /// Multi-step reasoning, code generation, complex analysis — use the best model.
    Complex,
}

impl std::fmt::Display for TaskComplexity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskComplexity::Simple => write!(f, "simple"),
            TaskComplexity::Medium => write!(f, "medium"),
            TaskComplexity::Complex => write!(f, "complex"),
        }
    }
}

/// Model router that selects the appropriate model based on query complexity.
#[derive(Debug, Clone)]
pub struct ModelRouter {
    config: ModelRoutingConfig,
}

impl ModelRouter {
    /// Create a new model router with the given routing configuration.
    pub fn new(config: ModelRoutingConfig) -> Self {
        Self { config }
    }

    /// Score a completion request and determine its complexity tier.
    ///
    /// Heuristics:
    /// - **Token count**: total characters in messages as a proxy for tokens
    /// - **Tool availability**: having tools suggests potential multi-step work
    /// - **Code markers**: backticks, `fn`, `def`, `class`, etc.
    /// - **Conversation depth**: more messages = more context = harder reasoning
    /// - **System prompt length**: longer prompts often imply complex tasks
    pub fn score(&self, request: &CompletionRequest) -> TaskComplexity {
        let mut score: u32 = 0;

        // 1. Total message content length (rough token proxy: ~4 chars per token)
        let total_chars: usize = request
            .messages
            .iter()
            .map(|m| m.content.text_length())
            .sum();
        let approx_tokens = (total_chars / 4) as u32;
        score += approx_tokens;

        // 2. Tool availability adds complexity
        let tool_count = request.tools.len() as u32;
        if tool_count > 0 {
            score += tool_count * 20;
        }

        // 3. Code markers in the last user message
        if let Some(last_msg) = request.messages.last() {
            let text = last_msg.content.text_content();
            let text_lower = text.to_lowercase();
            let code_markers = [
                "```",
                "fn ",
                "def ",
                "class ",
                "import ",
                "function ",
                "async ",
                "await ",
                "struct ",
                "impl ",
                "return ",
            ];
            let code_score: u32 = code_markers
                .iter()
                .filter(|marker| text_lower.contains(*marker))
                .count() as u32;
            score += code_score * 30;
        }

        // 4. Conversation depth
        let msg_count = request.messages.len() as u32;
        if msg_count > 10 {
            score += (msg_count - 10) * 15;
        }

        // 5. System prompt complexity
        if let Some(ref system) = request.system {
            let sys_len = system.len() as u32;
            if sys_len > 500 {
                score += (sys_len - 500) / 10;
            }
        }

        // Classify
        if score < self.config.simple_threshold {
            TaskComplexity::Simple
        } else if score >= self.config.complex_threshold {
            TaskComplexity::Complex
        } else {
            TaskComplexity::Medium
        }
    }

    /// Select the model name for a given complexity tier.
    pub fn model_for_complexity(&self, complexity: TaskComplexity) -> &str {
        match complexity {
            TaskComplexity::Simple => &self.config.simple_model,
            TaskComplexity::Medium => &self.config.medium_model,
            TaskComplexity::Complex => &self.config.complex_model,
        }
    }

    /// Score a request and return the selected model name + complexity.
    pub fn select_model(&self, request: &CompletionRequest) -> (TaskComplexity, String) {
        let complexity = self.score(request);
        let model = self.model_for_complexity(complexity).to_string();
        (complexity, model)
    }

    /// Return the raw numeric score for a request (for debugging/logging).
    pub fn raw_score(&self, request: &CompletionRequest) -> u32 {
        let mut score: u32 = 0;
        let total_chars: usize = request
            .messages
            .iter()
            .map(|m| m.content.text_length())
            .sum();
        score += (total_chars / 4) as u32;
        let tool_count = request.tools.len() as u32;
        if tool_count > 0 {
            score += tool_count * 20;
        }
        if let Some(last_msg) = request.messages.last() {
            let text = last_msg.content.text_content();
            let text_lower = text.to_lowercase();
            let code_markers = [
                "```", "fn ", "def ", "class ", "import ", "function ", "async ", "await ",
                "struct ", "impl ", "return ",
            ];
            score += code_markers
                .iter()
                .filter(|marker| text_lower.contains(*marker))
                .count() as u32
                * 30;
        }
        let msg_count = request.messages.len() as u32;
        if msg_count > 10 {
            score += (msg_count - 10) * 15;
        }
        if let Some(ref system) = request.system {
            let sys_len = system.len() as u32;
            if sys_len > 500 {
                score += (sys_len - 500) / 10;
            }
        }
        score
    }

    /// Validate that all configured models exist in the catalog.
    ///
    /// Returns a list of warning messages for models not found in the catalog.
    pub fn validate_models(&self, catalog: &crate::model_catalog::ModelCatalog) -> Vec<String> {
        let mut warnings = vec![];
        for model in [
            &self.config.simple_model,
            &self.config.medium_model,
            &self.config.complex_model,
        ] {
            if catalog.find_model(model).is_none() {
                warnings.push(format!("Model '{}' not found in catalog", model));
            }
        }
        warnings
    }

    /// Resolve aliases in the routing config using the catalog.
    ///
    /// For example, if "sonnet" is configured, resolves to "claude-sonnet-4-6".
    pub fn resolve_aliases(&mut self, catalog: &crate::model_catalog::ModelCatalog) {
        if let Some(resolved) = catalog.resolve_alias(&self.config.simple_model) {
            self.config.simple_model = resolved.to_string();
        }
        if let Some(resolved) = catalog.resolve_alias(&self.config.medium_model) {
            self.config.medium_model = resolved.to_string();
        }
        if let Some(resolved) = catalog.resolve_alias(&self.config.complex_model) {
            self.config.complex_model = resolved.to_string();
        }
    }
}

// ---------------------------------------------------------------------------
// Two-stage dynamic tool selection
// ---------------------------------------------------------------------------

/// Classification prompt used by the tool selector (Stage 1).
pub const TOOL_SELECTOR_SYSTEM_PROMPT: &str = "\
You are a message classifier. Given the user message, return a JSON array of relevant tool \
categories from: [greeting, os_control, research, trading, scheduling, coding, browser, media, \
agent_management, general]. If no tools are needed, return []. Only return the JSON array, nothing else.";

/// All known category names (used for validation).
pub const KNOWN_CATEGORIES: &[&str] = &[
    "greeting",
    "os_control",
    "research",
    "trading",
    "scheduling",
    "coding",
    "browser",
    "media",
    "agent_management",
    "personal",
    "general",
];

/// Dynamic tool selector — filters the full tool set based on LLM classification.
#[derive(Debug, Clone)]
pub struct ToolSelector {
    /// Model to use for the fast classification call.
    pub model: String,
    /// Category → tool name patterns (supports trailing `*` wildcard).
    pub tool_groups: HashMap<String, Vec<String>>,
}

impl ToolSelector {
    /// Create from a routing config.
    pub fn from_config(config: &ModelRoutingConfig) -> Self {
        Self {
            model: config.tool_selector_model.clone(),
            tool_groups: config.tool_groups.clone(),
        }
    }

    /// Parse the JSON array returned by the classifier.
    ///
    /// Returns `None` on parse failure (caller should fall back to all tools).
    pub fn parse_categories(response: &str) -> Option<Vec<String>> {
        // Try to find a JSON array in the response (the model might wrap it in markdown etc.)
        let trimmed = response.trim();

        // Fast path: response is just the array
        if let Ok(cats) = serde_json::from_str::<Vec<String>>(trimmed) {
            return Some(cats);
        }

        // Try extracting the first `[…]` block
        if let Some(start) = trimmed.find('[') {
            if let Some(end) = trimmed.rfind(']') {
                if end > start {
                    let bracket_content = &trimmed[start..=end];
                    // Try strict JSON first
                    if let Ok(cats) = serde_json::from_str::<Vec<String>>(bracket_content) {
                        return Some(cats);
                    }
                    // Fallback: handle unquoted category names like [research, general]
                    // Small local models often omit quotes around simple identifiers.
                    let inner = &trimmed[start + 1..end];
                    let cats: Vec<String> = inner
                        .split(',')
                        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !cats.is_empty()
                        && cats
                            .iter()
                            .all(|c| KNOWN_CATEGORIES.contains(&c.as_str()))
                    {
                        return Some(cats);
                    }
                }
            }
        }

        warn!(raw = %trimmed, "Tool selector returned unparseable response — falling back to all tools");
        None
    }

    /// Given selected categories, resolve them to tool name patterns.
    pub fn patterns_for_categories(&self, categories: &[String]) -> Vec<String> {
        let mut patterns = Vec::new();
        for cat in categories {
            if let Some(pats) = self.tool_groups.get(cat.as_str()) {
                for p in pats {
                    if !patterns.contains(p) {
                        patterns.push(p.clone());
                    }
                }
            }
        }
        patterns
    }

    /// Filter a tool list, keeping only tools whose name matches at least one pattern.
    ///
    /// Patterns support:
    /// - Exact match: `"web_search"` matches tool named `"web_search"`
    /// - Trailing wildcard: `"mcp_macos_*"` matches `"mcp_macos_open_app"`, `"mcp_macos_click"`, etc.
    pub fn filter_tools(tools: &[ToolDefinition], patterns: &[String]) -> Vec<ToolDefinition> {
        if patterns.is_empty() {
            return vec![];
        }
        tools
            .iter()
            .filter(|t| patterns.iter().any(|p| pattern_matches(p, &t.name)))
            .cloned()
            .collect()
    }

    /// Classify message into categories using simple keyword matching (no LLM call).
    ///
    /// This is a fast, zero-cost alternative to the LLM-based classification.
    /// Returns relevant categories based on keywords found in the message.
    ///
    /// Categories:
    /// - `trading`: trading, stock, market, buy, sell, portfolio, etc.
    /// - `personal`: expense, receipt, email, calendar, gmail, etc.
    /// - `coding`: code, bug, deploy, file, rust, python, etc.
    /// - `os_control`: open, close, click, screenshot, macos, etc.
    /// - `research`: search, web, fetch, knowledge, etc.
    /// - `browser`: browser, navigate, screenshot, etc.
    /// - `media`: image, audio, video, tts, stt, etc.
    /// - `agent_management`: agent, spawn, task, hand, etc.
    /// - `scheduling`: schedule, cron, reminder, etc.
    /// - `general`: fallback when no specific category matches
    pub fn classify_message_by_keywords(message: &str) -> Vec<String> {
        let msg_lower = message.to_lowercase();
        let mut categories = Vec::new();

        // Trading keywords
        let trading_keywords = [
            "trade", "trading", "stock", "buy", "sell", "portfolio", "market",
            "prediction", "paper_trade", "price", "ticker", "nvda", "aapl",
            "spy", "qqq", "crypto", "bitcoin", "eth", "investment", "position",
            "gate", "learning", "backtest"
        ];
        if trading_keywords.iter().any(|k| msg_lower.contains(k)) {
            categories.push("trading".to_string());
        }

        // Personal keywords
        let personal_keywords = [
            "expense", "receipt", "email", "gmail", "calendar", "order",
            "restaurant", "booking", "appointment", "event", "meeting",
            "schedule", "reminder"
        ];
        if personal_keywords.iter().any(|k| msg_lower.contains(k)) {
            categories.push("personal".to_string());
        }

        // Coding keywords
        let coding_keywords = [
            "code", "coding", "bug", "deploy", "rust", "python", "javascript",
            "file", "directory", "patch", "git", "commit", "build", "compile",
            "test", "cargo", "npm", "intent", "function", "class", "debug",
            "error", "exception", "refactor"
        ];
        if coding_keywords.iter().any(|k| msg_lower.contains(k)) {
            categories.push("coding".to_string());
        }

        // OS control keywords
        let os_keywords = [
            "open app", "close app", "click", "screenshot", "macos", "window",
            "minimize", "maximize", "finder", "safari", "chrome", "terminal"
        ];
        if os_keywords.iter().any(|k| msg_lower.contains(k)) {
            categories.push("os_control".to_string());
        }

        // Research keywords
        let research_keywords = [
            "search", "web", "fetch", "knowledge", "lookup", "find", "google",
            "research", "article", "wikipedia", "documentation", "docs"
        ];
        if research_keywords.iter().any(|k| msg_lower.contains(k)) {
            categories.push("research".to_string());
        }

        // Browser keywords
        let browser_keywords = [
            "browser", "navigate", "url", "webpage", "website", "http",
            "click button", "fill form", "scrape"
        ];
        if browser_keywords.iter().any(|k| msg_lower.contains(k)) {
            categories.push("browser".to_string());
        }

        // Media keywords
        let media_keywords = [
            "image", "photo", "picture", "audio", "video", "media",
            "tts", "text to speech", "speech to text", "stt", "speak",
            "read aloud", "transcribe"
        ];
        if media_keywords.iter().any(|k| msg_lower.contains(k)) {
            categories.push("media".to_string());
        }

        // Agent management keywords
        let agent_keywords = [
            "agent", "spawn", "task", "delegate", "hand", "supervisor",
            "create agent", "kill agent", "list agents"
        ];
        if agent_keywords.iter().any(|k| msg_lower.contains(k)) {
            categories.push("agent_management".to_string());
        }

        // Scheduling keywords (different from personal calendar)
        let scheduling_keywords = [
            "cron", "scheduled task", "recurring", "every day", "every hour",
            "background job", "periodic"
        ];
        if scheduling_keywords.iter().any(|k| msg_lower.contains(k)) {
            categories.push("scheduling".to_string());
        }

        // If no specific category matched, use general
        if categories.is_empty() {
            categories.push("general".to_string());
        }

        categories
    }
}

/// Check if a pattern (possibly with trailing `*`) matches a tool name.
fn pattern_matches(pattern: &str, name: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        name == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openfang_types::message::{Message, MessageContent, Role};
    use openfang_types::tool::ToolDefinition;

    fn default_config() -> ModelRoutingConfig {
        ModelRoutingConfig {
            simple_model: "llama-3.3-70b-versatile".to_string(),
            medium_model: "claude-sonnet-4-6".to_string(),
            complex_model: "claude-opus-4-6".to_string(),
            simple_threshold: 200,
            complex_threshold: 800,
            ..Default::default()
        }
    }

    fn make_request(messages: Vec<Message>, tools: Vec<ToolDefinition>) -> CompletionRequest {
        CompletionRequest {
            model: "placeholder".to_string(),
            messages,
            tools,
            max_tokens: 4096,
            temperature: 0.7,
            system: None,
            thinking: None,
            priority: Default::default(),
        }
    }

    #[test]
    fn test_simple_greeting_routes_to_simple() {
        let router = ModelRouter::new(default_config());
        let request = make_request(
            vec![Message {
                role: Role::User,
                content: MessageContent::text("Hello!"),
            }],
            vec![],
        );
        let (complexity, model) = router.select_model(&request);
        assert_eq!(complexity, TaskComplexity::Simple);
        assert_eq!(model, "llama-3.3-70b-versatile");
    }

    #[test]
    fn test_code_markers_increase_complexity() {
        let router = ModelRouter::new(default_config());
        let request = make_request(
            vec![Message {
                role: Role::User,
                content: MessageContent::text(
                    "Write a function that implements async file reading with struct and impl blocks:\n\
                     ```rust\nfn main() { }\n```"
                ),
            }],
            vec![],
        );
        let complexity = router.score(&request);
        // Should be at least Medium due to code markers
        assert_ne!(complexity, TaskComplexity::Simple);
    }

    #[test]
    fn test_tools_increase_complexity() {
        let router = ModelRouter::new(default_config());
        let tools: Vec<ToolDefinition> = (0..15)
            .map(|i| ToolDefinition {
                name: format!("tool_{i}"),
                description: "A test tool".to_string(),
                input_schema: serde_json::json!({}),
            })
            .collect();
        let request = make_request(
            vec![Message {
                role: Role::User,
                content: MessageContent::text("Use the available tools to solve this problem."),
            }],
            tools,
        );
        let complexity = router.score(&request);
        // 15 tools * 20 = 300 — should be at least Medium
        assert_ne!(complexity, TaskComplexity::Simple);
    }

    #[test]
    fn test_long_conversation_routes_higher() {
        let router = ModelRouter::new(default_config());
        // 20 messages with moderate content
        let messages: Vec<Message> = (0..20)
            .map(|i| Message {
                role: if i % 2 == 0 { Role::User } else { Role::Assistant },
                content: MessageContent::text(format!(
                    "This is message {} with enough content to add some token weight to the conversation.",
                    i
                )),
            })
            .collect();
        let request = make_request(messages, vec![]);
        let complexity = router.score(&request);
        // Long conversation should be Medium or Complex
        assert_ne!(complexity, TaskComplexity::Simple);
    }

    #[test]
    fn test_model_for_complexity() {
        let router = ModelRouter::new(default_config());
        assert_eq!(
            router.model_for_complexity(TaskComplexity::Simple),
            "llama-3.3-70b-versatile"
        );
        assert_eq!(
            router.model_for_complexity(TaskComplexity::Medium),
            "claude-sonnet-4-6"
        );
        assert_eq!(
            router.model_for_complexity(TaskComplexity::Complex),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn test_complexity_display() {
        assert_eq!(TaskComplexity::Simple.to_string(), "simple");
        assert_eq!(TaskComplexity::Medium.to_string(), "medium");
        assert_eq!(TaskComplexity::Complex.to_string(), "complex");
    }

    #[test]
    fn test_validate_models_all_found() {
        let catalog = crate::model_catalog::ModelCatalog::new();
        let config = ModelRoutingConfig {
            simple_model: "llama-3.3-70b-versatile".to_string(),
            medium_model: "claude-sonnet-4-6".to_string(),
            complex_model: "claude-opus-4-6".to_string(),
            simple_threshold: 200,
            complex_threshold: 800,
            ..Default::default()
        };
        let router = ModelRouter::new(config);
        let warnings = router.validate_models(&catalog);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_validate_models_unknown() {
        let catalog = crate::model_catalog::ModelCatalog::new();
        let config = ModelRoutingConfig {
            simple_model: "unknown-model".to_string(),
            medium_model: "claude-sonnet-4-6".to_string(),
            complex_model: "claude-opus-4-6".to_string(),
            simple_threshold: 200,
            complex_threshold: 800,
            ..Default::default()
        };
        let router = ModelRouter::new(config);
        let warnings = router.validate_models(&catalog);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unknown-model"));
    }

    #[test]
    fn test_resolve_aliases() {
        let catalog = crate::model_catalog::ModelCatalog::new();
        let config = ModelRoutingConfig {
            simple_model: "llama".to_string(),
            medium_model: "sonnet".to_string(),
            complex_model: "opus".to_string(),
            simple_threshold: 200,
            complex_threshold: 800,
            ..Default::default()
        };
        let mut router = ModelRouter::new(config);
        router.resolve_aliases(&catalog);
        assert_eq!(
            router.model_for_complexity(TaskComplexity::Simple),
            "llama-3.3-70b-versatile"
        );
        assert_eq!(
            router.model_for_complexity(TaskComplexity::Medium),
            "claude-sonnet-4-6"
        );
        assert_eq!(
            router.model_for_complexity(TaskComplexity::Complex),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn test_system_prompt_adds_complexity() {
        let router = ModelRouter::new(default_config());
        let mut request = make_request(
            vec![Message {
                role: Role::User,
                content: MessageContent::text("Hi"),
            }],
            vec![],
        );
        request.system = Some("A".repeat(2000)); // Long system prompt
        let complexity_with_long_system = router.score(&request);

        let mut request2 = make_request(
            vec![Message {
                role: Role::User,
                content: MessageContent::text("Hi"),
            }],
            vec![],
        );
        request2.system = Some("Be helpful.".to_string());
        let complexity_short = router.score(&request2);

        // Long system prompt should score higher or equal
        assert!(complexity_with_long_system as u32 >= complexity_short as u32);
    }

    // ── ToolSelector tests ─────────────────────────────────────────────

    #[test]
    fn test_pattern_matches_exact() {
        assert!(pattern_matches("web_search", "web_search"));
        assert!(!pattern_matches("web_search", "web_fetch"));
    }

    #[test]
    fn test_pattern_matches_wildcard() {
        assert!(pattern_matches("mcp_macos_*", "mcp_macos_open_app"));
        assert!(pattern_matches("mcp_macos_*", "mcp_macos_click"));
        assert!(pattern_matches("mcp_macos_*", "mcp_macos_"));
        assert!(!pattern_matches("mcp_macos_*", "mcp_linux_open_app"));
        assert!(!pattern_matches("mcp_macos_*", "web_search"));
    }

    #[test]
    fn test_parse_categories_valid_json() {
        let cats = ToolSelector::parse_categories(r#"["greeting", "os_control"]"#).unwrap();
        assert_eq!(cats, vec!["greeting", "os_control"]);
    }

    #[test]
    fn test_parse_categories_empty_array() {
        let cats = ToolSelector::parse_categories("[]").unwrap();
        assert!(cats.is_empty());
    }

    #[test]
    fn test_parse_categories_wrapped_in_markdown() {
        let response = "```json\n[\"research\", \"general\"]\n```";
        let cats = ToolSelector::parse_categories(response).unwrap();
        assert_eq!(cats, vec!["research", "general"]);
    }

    #[test]
    fn test_parse_categories_with_preamble() {
        let response = "Based on the message, the categories are: [\"coding\"]";
        let cats = ToolSelector::parse_categories(response).unwrap();
        assert_eq!(cats, vec!["coding"]);
    }

    #[test]
    fn test_parse_categories_invalid() {
        assert!(ToolSelector::parse_categories("not json at all").is_none());
        assert!(ToolSelector::parse_categories("").is_none());
    }

    #[test]
    fn test_parse_categories_unquoted() {
        // Small local models often return unquoted category names
        let cats = ToolSelector::parse_categories("[research]").unwrap();
        assert_eq!(cats, vec!["research"]);
    }

    #[test]
    fn test_parse_categories_unquoted_multiple() {
        let cats = ToolSelector::parse_categories("[research, general]").unwrap();
        assert_eq!(cats, vec!["research", "general"]);
    }

    #[test]
    fn test_parse_categories_unquoted_unknown_rejected() {
        // Unknown categories should not be accepted via the unquoted path
        assert!(ToolSelector::parse_categories("[unknown_category]").is_none());
    }

    #[test]
    fn test_filter_tools_exact_match() {
        let tools = vec![
            ToolDefinition { name: "web_search".into(), description: String::new(), input_schema: serde_json::json!({}) },
            ToolDefinition { name: "web_fetch".into(), description: String::new(), input_schema: serde_json::json!({}) },
            ToolDefinition { name: "file_read".into(), description: String::new(), input_schema: serde_json::json!({}) },
        ];
        let patterns = vec!["web_search".to_string()];
        let filtered = ToolSelector::filter_tools(&tools, &patterns);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "web_search");
    }

    #[test]
    fn test_filter_tools_wildcard() {
        let tools = vec![
            ToolDefinition { name: "mcp_macos_open".into(), description: String::new(), input_schema: serde_json::json!({}) },
            ToolDefinition { name: "mcp_macos_click".into(), description: String::new(), input_schema: serde_json::json!({}) },
            ToolDefinition { name: "mcp_sshbox_exec".into(), description: String::new(), input_schema: serde_json::json!({}) },
            ToolDefinition { name: "web_search".into(), description: String::new(), input_schema: serde_json::json!({}) },
        ];
        let patterns = vec!["mcp_macos_*".to_string()];
        let filtered = ToolSelector::filter_tools(&tools, &patterns);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|t| t.name.starts_with("mcp_macos_")));
    }

    #[test]
    fn test_filter_tools_empty_patterns_returns_empty() {
        let tools = vec![
            ToolDefinition { name: "web_search".into(), description: String::new(), input_schema: serde_json::json!({}) },
        ];
        let filtered = ToolSelector::filter_tools(&tools, &[]);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_patterns_for_categories() {
        let selector = ToolSelector::from_config(&ModelRoutingConfig::default());
        let patterns = selector.patterns_for_categories(&["research".to_string()]);
        assert!(patterns.contains(&"web_search".to_string()));
        assert!(patterns.contains(&"web_fetch".to_string()));
        assert!(patterns.contains(&"memory_*".to_string()));
    }

    #[test]
    fn test_greeting_category_returns_no_patterns() {
        let selector = ToolSelector::from_config(&ModelRoutingConfig::default());
        let patterns = selector.patterns_for_categories(&["greeting".to_string()]);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_multiple_categories_deduplicate() {
        let selector = ToolSelector::from_config(&ModelRoutingConfig::default());
        // Both research and general include "web_search" and "memory_*"
        let patterns = selector.patterns_for_categories(&[
            "research".to_string(),
            "general".to_string(),
        ]);
        let web_search_count = patterns.iter().filter(|p| p.as_str() == "web_search").count();
        assert_eq!(web_search_count, 1, "web_search should not be duplicated");
    }

    #[test]
    fn test_default_tool_groups_has_all_known_categories() {
        let config = ModelRoutingConfig::default();
        for cat in KNOWN_CATEGORIES {
            assert!(
                config.tool_groups.contains_key(*cat),
                "Missing category '{}' in default tool_groups",
                cat
            );
        }
    }

    // ── Keyword classification tests ───────────────────────────────────

    #[test]
    fn test_classify_trading_message() {
        let categories = ToolSelector::classify_message_by_keywords("buy 100 shares of NVDA");
        assert!(categories.contains(&"trading".to_string()));
    }

    #[test]
    fn test_classify_personal_message() {
        let categories = ToolSelector::classify_message_by_keywords("check my email and calendar");
        assert!(categories.contains(&"personal".to_string()));
    }

    #[test]
    fn test_classify_coding_message() {
        let categories = ToolSelector::classify_message_by_keywords("fix the bug in the rust code");
        assert!(categories.contains(&"coding".to_string()));
    }

    #[test]
    fn test_classify_research_message() {
        let categories = ToolSelector::classify_message_by_keywords("search the web for information");
        assert!(categories.contains(&"research".to_string()));
    }

    #[test]
    fn test_classify_os_control_message() {
        let categories = ToolSelector::classify_message_by_keywords("open app Safari");
        assert!(categories.contains(&"os_control".to_string()));
    }

    #[test]
    fn test_classify_browser_message() {
        let categories = ToolSelector::classify_message_by_keywords("navigate to https://example.com");
        assert!(categories.contains(&"browser".to_string()));
    }

    #[test]
    fn test_classify_general_fallback() {
        let categories = ToolSelector::classify_message_by_keywords("hello how are you");
        assert_eq!(categories, vec!["general"]);
    }

    #[test]
    fn test_classify_multiple_categories() {
        let categories = ToolSelector::classify_message_by_keywords(
            "search for stock prices and check my email"
        );
        assert!(categories.contains(&"research".to_string()));
        assert!(categories.contains(&"personal".to_string()));
        assert!(categories.contains(&"trading".to_string()));
    }

    #[test]
    fn test_classify_case_insensitive() {
        let categories = ToolSelector::classify_message_by_keywords("BUY AAPL STOCK");
        assert!(categories.contains(&"trading".to_string()));
    }
}
