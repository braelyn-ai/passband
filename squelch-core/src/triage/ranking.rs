//! One ranking function for every consumer of already-selected FYE attention.
use super::{agent_config::RankingConfig, decision::AttentionFactors};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RankComponents {
    pub urgency: f64,
    pub action_need: f64,
    pub personal_relevance: f64,
    pub recency: f64,
    pub waiting: f64,
    pub importance: f64,
    /// Raw: the model's machine-written likelihood. Contribution: a penalty
    /// (never positive) that fades as action need or personal relevance rise.
    #[serde(default)]
    pub ai_generated: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankBreakdown {
    pub raw: RankComponents,
    pub contributions: RankComponents,
    pub total: f64,
    pub evaluated_at: DateTime<Utc>,
    /// Actual nonsecret settings make historical scores reproducible.
    pub config: RankingConfig,
}
fn unit(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}
pub fn rank(
    factors: &AttentionFactors,
    relevant_activity: DateTime<Utc>,
    unresolved_since: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    config: &RankingConfig,
) -> RankBreakdown {
    let hours = (now - relevant_activity).num_seconds().max(0) as f64 / 3600.0;
    let days = unresolved_since
        .map(|since| (now - since).num_seconds().max(0) as f64 / 86400.0)
        .unwrap_or(0.0);
    let raw = RankComponents {
        urgency: unit(factors.urgency),
        action_need: unit(factors.action_need),
        personal_relevance: unit(factors.personal_relevance),
        importance: unit(factors.importance),
        recency: unit(2f64.powf(-hours / config.recency_half_life_hours)),
        waiting: unit(days / config.waiting_saturation_days),
        ai_generated: unit(factors.ai_generated),
    };
    // Slop from a stranger sinks; slop from someone who needs an answer does
    // not. Whichever of action need or personal relevance is higher shields
    // the thread from the penalty in proportion.
    let shield = raw.action_need.max(raw.personal_relevance);
    let contributions = RankComponents {
        urgency: raw.urgency * config.urgency_weight,
        action_need: raw.action_need * config.action_need_weight,
        personal_relevance: raw.personal_relevance * config.personal_relevance_weight,
        recency: raw.recency * config.recency_weight,
        waiting: raw.waiting * config.waiting_weight,
        importance: raw.importance * config.importance_weight,
        ai_generated: -(raw.ai_generated * (1.0 - shield) * config.ai_generated_penalty_weight),
    };
    let total = contributions.urgency
        + contributions.action_need
        + contributions.personal_relevance
        + contributions.recency
        + contributions.waiting
        + contributions.importance
        + contributions.ai_generated;
    RankBreakdown {
        raw,
        contributions,
        total,
        evaluated_at: now,
        config: config.clone(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    #[test]
    fn recency_is_halved_after_one_day_without_retriage_clock() {
        let now = Utc::now();
        let cfg = RankingConfig::default();
        let factors = AttentionFactors::default();
        assert_eq!(
            rank(&factors, now, None, now, &cfg).contributions.recency,
            20.0
        );
        assert_eq!(
            rank(&factors, now - Duration::hours(24), None, now, &cfg)
                .contributions
                .recency,
            10.0
        );
    }
    #[test]
    fn old_obligation_can_outrank_fresh_informational_mail() {
        let now = Utc::now();
        let cfg = RankingConfig::default();
        let old = AttentionFactors {
            urgency: 1.0,
            action_need: 1.0,
            ..Default::default()
        };
        let fresh = AttentionFactors {
            importance: 1.0,
            ..Default::default()
        };
        assert!(
            rank(
                &old,
                now - Duration::days(30),
                Some(now - Duration::days(30)),
                now,
                &cfg
            )
            .total
                > rank(&fresh, now, None, now, &cfg).total
        );
    }
    #[test]
    fn machine_written_cold_mail_sinks_below_equivalent_human_mail() {
        let now = Utc::now();
        let cfg = RankingConfig::default();
        let human = AttentionFactors {
            personal_relevance: 0.2,
            ..Default::default()
        };
        let slop = AttentionFactors {
            ai_generated: 0.9,
            ..human.clone()
        };
        let a = rank(&human, now, None, now, &cfg);
        let b = rank(&slop, now, None, now, &cfg);
        assert!(b.total < a.total);
        assert!(b.contributions.ai_generated < 0.0);
    }
    #[test]
    fn ai_drafted_mail_that_needs_the_user_is_not_penalized() {
        let now = Utc::now();
        let cfg = RankingConfig::default();
        let human = AttentionFactors {
            action_need: 1.0,
            ..Default::default()
        };
        let drafted = AttentionFactors {
            ai_generated: 1.0,
            ..human.clone()
        };
        assert_eq!(
            rank(&human, now, None, now, &cfg).total,
            rank(&drafted, now, None, now, &cfg).total
        );
    }
    #[test]
    fn stored_breakdowns_without_ai_component_still_decode() {
        let json = r#"{"urgency":1,"action_need":0,"personal_relevance":0,
            "recency":0,"waiting":0,"importance":0}"#;
        let parsed: RankComponents = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.ai_generated, 0.0);
        let factors: AttentionFactors = serde_json::from_str(
            r#"{"urgency":0,"action_need":0,"personal_relevance":0,"importance":0,"attention_at":null}"#,
        )
        .unwrap();
        assert_eq!(factors.ai_generated, 0.0);
    }
    #[test]
    fn invalid_time_scales_are_rejected() {
        assert!(
            RankingConfig {
                recency_half_life_hours: 0.0,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}
