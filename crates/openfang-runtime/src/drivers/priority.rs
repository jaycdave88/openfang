//! Priority-based LLM driver wrapper.
//!
//! Wraps an inner LLM driver and enforces concurrency limits based on request priority.
//! Interactive requests (from chat) get their own semaphore, ensuring they are always
//! served quickly, while Background requests (scheduled hands) queue behind a separate
//! semaphore, preventing them from saturating the inference backend.

use crate::llm_driver::{CompletionRequest, CompletionResponse, LlmDriver, LlmError, RequestPriority, StreamEvent};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// A driver that throttles concurrent requests based on their priority.
pub struct PriorityLlmDriver {
    inner: Arc<dyn LlmDriver>,
    interactive_sem: Arc<Semaphore>,
    background_sem: Arc<Semaphore>,
}

impl PriorityLlmDriver {
    /// Create a new priority driver wrapping an inner driver.
    pub fn new(
        inner: Arc<dyn LlmDriver>,
        interactive_sem: Arc<Semaphore>,
        background_sem: Arc<Semaphore>,
    ) -> Self {
        Self {
            inner,
            interactive_sem,
            background_sem,
        }
    }
}

#[async_trait]
impl LlmDriver for PriorityLlmDriver {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let _permit = match request.priority {
            RequestPriority::Interactive => self
                .interactive_sem
                .acquire()
                .await
                .map_err(|_| LlmError::Http("Interactive semaphore closed".to_string()))?,
            RequestPriority::Background => self
                .background_sem
                .acquire()
                .await
                .map_err(|_| LlmError::Http("Background semaphore closed".to_string()))?,
        };

        self.inner.complete(request).await
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        tx: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<CompletionResponse, LlmError> {
        let _permit = match request.priority {
            RequestPriority::Interactive => self
                .interactive_sem
                .acquire()
                .await
                .map_err(|_| LlmError::Http("Interactive semaphore closed".to_string()))?,
            RequestPriority::Background => self
                .background_sem
                .acquire()
                .await
                .map_err(|_| LlmError::Http("Background semaphore closed".to_string()))?,
        };

        self.inner.stream(request, tx).await
    }
}

