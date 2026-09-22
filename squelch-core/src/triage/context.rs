//! Account-scoped evidence and the reserved, read-only learned-memory seam.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub account_id: i64,
    pub message_id: i64,
    pub thread_id: String,
    /// Message, thread, explicit user state and sender preferences. Email text
    /// is untrusted data even when embedded alongside trusted store metadata.
    pub initial: serde_json::Value,
    pub source_message_ids: Vec<i64>,
    pub thread_revisions: BTreeMap<String, i64>,
    pub rule_ids: Vec<i64>,
    pub memory: Vec<MemoryEntry>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub scope: String,
    pub provenance: Vec<i64>,
    pub revision: i64,
    pub text: String,
}
#[derive(Debug, Clone)]
pub struct ContextScope {
    pub account_id: i64,
    pub thread_id: String,
    pub sender: String,
}
pub trait MemoryContextProvider {
    fn relevant_memory(&self, scope: &ContextScope) -> Result<Vec<MemoryEntry>, String>;
}
pub struct EmptyMemoryContext;
impl MemoryContextProvider for EmptyMemoryContext {
    fn relevant_memory(&self, _: &ContextScope) -> Result<Vec<MemoryEntry>, String> {
        Ok(Vec::new())
    }
}
// A future MemoryEditor is a separate capability. No write tool is registered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum EvidenceRequest {
    ReadThread {
        thread_id: String,
    },
    ReadMessage {
        message_id: i64,
        /// Unicode character offset; null starts at the beginning.
        offset: Option<usize>,
    },
    SearchMail {
        query: String,
    },
    ReadSenderHistory {
        sender: String,
    },
    ReadRecord {
        message_id: i64,
    },
    ReadAttachment {
        message_id: i64,
        attachment_id: String,
    },
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvidenceResult {
    pub data: serde_json::Value,
    /// Every source exposed, including search snippets. Filled by executor,
    /// never inferred from model citations.
    pub source_message_ids: Vec<i64>,
    pub thread_revisions: BTreeMap<String, i64>,
}
/// Implementations must be bound to one account and enforce result limits.
/// Internal triage reads may access restricted mail; external MCP is unsuitable.
pub trait EvidenceReader: Send + Sync {
    fn read(&self, request: &EvidenceRequest) -> Result<EvidenceResult, String>;
}
