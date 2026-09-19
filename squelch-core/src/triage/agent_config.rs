//! Central tuning levers. Scores change ordering, never destination membership.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentTriageConfig {
    pub agent: AgentConfig,
    pub context: ContextConfig,
    pub ranking: RankingConfig,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// None uses the deployment's resolved triage model.
    pub model: Option<String>,
    pub review_model: Option<String>,
    pub max_model_turns: usize,
    pub max_tool_calls: usize,
    pub max_review_calls: usize,
    pub timeout_secs: u64,
    pub concurrency: usize,
    /// Queue polling is independent from Gmail polling.
    pub worker_poll_secs: u64,
    /// Maximum investigations per account per UTC day; each run has its own turn cap.
    pub daily_run_cap: u32,
    /// Background investigations share the total cap but cannot consume the
    /// foreground reserve. Zero pauses background work; notification is separate.
    pub background_daily_run_cap: u32,
    /// Failed investigations remain inspectable and can be explicitly retried.
    pub max_attempts: u32,
    /// Shared provider-outage circuit cooldown. Outages do not consume attempts.
    pub outage_retry_secs: u64,
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: None,
            review_model: None,
            max_model_turns: 4,
            max_tool_calls: 8,
            max_review_calls: 1,
            timeout_secs: 90,
            concurrency: 2,
            worker_poll_secs: 1,
            daily_run_cap: 1000,
            background_daily_run_cap: 200,
            max_attempts: 6,
            outage_retry_secs: 300,
        }
    }
}
impl AgentConfig {
    /// Always reserve at least one run for first classification when operators
    /// lower only the total cap. Both effective values are exposed by the API.
    pub fn effective_background_daily_run_cap(&self) -> u32 {
        self.background_daily_run_cap
            .min(self.daily_run_cap.saturating_sub(1))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    pub initial_thread_messages: usize,
    pub max_related_messages: usize,
    pub max_context_bytes: usize,
    pub max_tool_result_bytes: usize,
}
impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            initial_thread_messages: 8,
            max_related_messages: 20,
            max_context_bytes: 120_000,
            max_tool_result_bytes: 24_000,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RankingConfig {
    pub urgency_weight: f64,
    pub action_need_weight: f64,
    pub personal_relevance_weight: f64,
    pub recency_weight: f64,
    pub waiting_weight: f64,
    pub importance_weight: f64,
    pub recency_half_life_hours: f64,
    pub waiting_saturation_days: f64,
}
impl Default for RankingConfig {
    fn default() -> Self {
        Self {
            urgency_weight: 30.0,
            action_need_weight: 25.0,
            personal_relevance_weight: 20.0,
            recency_weight: 20.0,
            waiting_weight: 5.0,
            importance_weight: 2.0,
            recency_half_life_hours: 24.0,
            waiting_saturation_days: 7.0,
        }
    }
}
impl AgentTriageConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.agent.daily_run_cap == 0
            || self.agent.max_attempts == 0
            || self.agent.max_model_turns == 0
            || self.agent.timeout_secs == 0
            || self.agent.concurrency == 0
            || self.agent.worker_poll_secs == 0
            || self.agent.outage_retry_secs == 0
            || self.context.max_context_bytes == 0
            || self.context.max_tool_result_bytes == 0
        {
            return Err("triage budgets must be positive".into());
        }
        self.ranking.validate()
    }
}
impl RankingConfig {
    pub fn validate(&self) -> Result<(), String> {
        if [
            self.urgency_weight,
            self.action_need_weight,
            self.personal_relevance_weight,
            self.recency_weight,
            self.waiting_weight,
            self.importance_weight,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err("ranking weights must be finite and nonnegative".into());
        }
        if [self.recency_half_life_hours, self.waiting_saturation_days]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err("ranking time scales must be finite and positive".into());
        }
        Ok(())
    }
}
