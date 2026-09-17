//! Durable jobs and human-facing projections for agent-owned triage.
//!
//! Storage validates concurrency and ownership. It does not classify mail.
use crate::triage::decision::{MessageDecision, ThreadAttentionDecision};
use crate::{error::Result, types::AccountId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentJob {
    pub id: i64,
    pub account_id: AccountId,
    pub message_id: i64,
    pub kind: String,
    pub trigger: String,
    pub lease_token: String,
    pub attempts: i64,
    pub arrival_eligible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub source: AgentSourceSnapshot,
    pub id: i64,
    pub thread_id: String,
    pub from_addr: String,
    pub subject: String,
    pub body: String,
    pub received_at: String,
    pub is_sent: bool,
    pub is_spam: bool,
    pub status: String,
    pub notify_eligible_at: Option<String>,
    pub opened_at: Option<String>,
    pub remind_at: Option<String>,
}

/// Captured with the evidence read and checked before any decision writes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSourceSnapshot {
    pub message_id: i64,
    pub content: String,
    pub user_state: String,
    pub attention_revision: i64,
}

/// Opaque snapshots compared inside the commit transaction. These include the
/// actual content and explicit state, so all existing user-action APIs invalidate
/// stale work without requiring each caller to remember a revision increment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextRevision {
    pub content: String,
    pub preferences: String,
    pub user_state: String,
    pub attention_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContext {
    pub message: AgentMessage,
    pub thread: Vec<AgentMessage>,
    pub previous_decision: Option<MessageDecision>,
    pub attention: Option<ThreadAttentionDecision>,
    pub rules: Vec<serde_json::Value>,
    pub matched_rules: Vec<serde_json::Value>,
    pub sender_is_contact: bool,
    pub corrections: Vec<serde_json::Value>,
    pub revision: ContextRevision,
    /// Reserved read-only seam. No learned-memory editor ships in this rewrite.
    pub memory: Vec<serde_json::Value>,
    /// Executor-owned model, prompt, configuration and usage metadata.
    pub run_metadata: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCommitOutcome {
    Applied,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentListItem {
    pub message_id: i64,
    pub decision_source_message_id: i64,
    pub thread_id: String,
    pub from_addr: String,
    pub subject: String,
    /// Effective list activity, capped at local ingestion; raw source dates
    /// remain available on AgentMessage and the human thread reader.
    pub received_at: String,
    pub decision: MessageDecision,
    pub attention: ThreadAttentionDecision,
    pub score: f64,
    pub unresolved_since: Option<String>,
    pub ranking: Option<crate::triage::ranking::RankBreakdown>,
}

/// Internal context reads are account scoped, but may inspect restricted mail.
/// External callers must use `agent_access_allowed` before exposing a source.
pub trait AgentTriageStore: Send + Sync {
    /// Reserve one bounded investigation in every scope atomically. A rejection
    /// charges none of the scopes; caps count investigations, not model turns.
    fn reserve_agent_budget(
        &self,
        account: AccountId,
        day: &str,
        limits: &[(String, u32)],
    ) -> Result<bool>;
    /// Refund a reservation rejected before any model work was accepted.
    fn refund_agent_budget(
        &self,
        account: AccountId,
        day: &str,
        limits: &[(String, u32)],
    ) -> Result<()>;

    fn agent_diagnostics(&self, account: AccountId, message: i64) -> Result<serde_json::Value>;
    fn human_agent_updates(
        &self,
        account: AccountId,
        query: &AgentInventoryQuery,
    ) -> Result<Vec<crate::types::AttentionUpdate>>;
    fn external_agent_fye(
        &self,
        account: AccountId,
        limit: usize,
        config: &crate::triage::agent_config::RankingConfig,
        now: DateTime<Utc>,
    ) -> Result<Vec<AgentListItem>>;
    fn external_agent_reading(
        &self,
        account: AccountId,
        limit: usize,
    ) -> Result<Vec<AgentListItem>>;
    fn external_agent_decision(&self, account: AccountId, message: i64) -> Result<MessageDecision>;
    fn external_agent_records(
        &self,
        account: AccountId,
        limit: usize,
    ) -> Result<Vec<AgentListItem>>;
    fn correct_agent_triage(
        &self,
        account: AccountId,
        message: i64,
        field: &str,
        value: &serde_json::Value,
        now: DateTime<Utc>,
    ) -> Result<()>;
    fn enqueue_agent_triage(
        &self,
        account: AccountId,
        message: i64,
        trigger: &str,
        arrival_eligible: bool,
    ) -> Result<()>;
    /// `investigation` claims full triage and access assessments;
    /// `initial_investigation` excludes autonomous revisits.
    fn claim_agent_job(
        &self,
        account: AccountId,
        kind: &str,
        now: DateTime<Utc>,
        lease_seconds: i64,
    ) -> Result<Option<AgentJob>>;
    fn load_agent_context(&self, job: &AgentJob) -> Result<AgentContext>;
    fn commit_agent_decision(
        &self,
        job: &AgentJob,
        context: &AgentContext,
        decision: &MessageDecision,
        sources: &[AgentSourceSnapshot],
    ) -> Result<AgentCommitOutcome>;
    fn commit_agent_decision_with_policy(
        &self,
        job: &AgentJob,
        context: &AgentContext,
        decision: &MessageDecision,
        sources: &[AgentSourceSnapshot],
        policy: &crate::config::RevisitPassConfig,
    ) -> Result<AgentCommitOutcome>;
    fn correct_agent_triage_delta(
        &self,
        account: AccountId,
        message: i64,
        field: &str,
        add: &[String],
        remove: &[String],
        now: DateTime<Utc>,
    ) -> Result<()>;
    fn complete_agent_job(&self, job: &AgentJob) -> Result<bool>;
    fn fail_agent_job(&self, job: &AgentJob, error_code: &str) -> Result<bool>;
    fn retry_agent_job(
        &self,
        job: &AgentJob,
        retry_at: DateTime<Utc>,
        error_code: &str,
    ) -> Result<bool>;
    /// Requeue work that could not start, refunding only this lease's attempt.
    fn defer_agent_job(
        &self,
        job: &AgentJob,
        available_at: DateTime<Utc>,
        error_code: &str,
    ) -> Result<bool>;
    fn agent_fye(
        &self,
        account: AccountId,
        limit: usize,
        config: &crate::triage::agent_config::RankingConfig,
        now: DateTime<Utc>,
    ) -> Result<Vec<AgentListItem>>;
    fn acknowledge_agent_message(
        &self,
        account: AccountId,
        message: i64,
        now: DateTime<Utc>,
    ) -> Result<()>;
    fn agent_records(&self, account: AccountId, limit: usize) -> Result<Vec<AgentListItem>>;
    fn agent_reading(&self, account: AccountId, limit: usize) -> Result<Vec<AgentListItem>>;
    fn agent_access_allowed(&self, account: AccountId, message: i64) -> Result<bool>;
    fn snapshot_agent_sources(
        &self,
        account: AccountId,
        ids: &[i64],
    ) -> Result<Vec<AgentSourceSnapshot>>;
    fn agent_thread_context(&self, account: AccountId, thread: &str) -> Result<AgentContext>;
    fn agent_read_message(&self, account: AccountId, message: i64) -> Result<AgentMessage>;
    fn agent_read_thread(
        &self,
        account: AccountId,
        thread: &str,
        limit: usize,
    ) -> Result<Vec<AgentMessage>>;
    fn agent_search_mail(
        &self,
        account: AccountId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<AgentMessage>>;
    fn agent_sender_history(
        &self,
        account: AccountId,
        sender: &str,
        limit: usize,
    ) -> Result<Vec<AgentMessage>>;
    /// One-time, push-silent historical queue. Existing explicit user state is untouched.
    fn initialize_agent_cutover(&self, account: AccountId, since: DateTime<Utc>) -> Result<usize>;
}

/// Filters for the human mail inventory. None of these grant external access.
#[derive(Debug, Clone)]
pub struct AgentInventoryQuery {
    pub since: DateTime<Utc>,
    pub min_importance: Option<u8>,
    pub status: Option<crate::types::AttentionStatus>,
    pub band: Option<crate::store::SitrepBand>,
    pub pending_reminders: bool,
    pub spam: crate::store::SpamScope,
    pub ranking: crate::triage::agent_config::RankingConfig,
}
