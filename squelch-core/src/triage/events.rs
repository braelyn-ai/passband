//! Shared arrival-notification qualification. Model-assessed auth always qualifies;
//! other mail uses the configured notification importance threshold. Transport
//! freshness, origin eligibility, and delivery dedup remain lifecycle concerns.
//! Legacy context fields stay temporarily for retiring call sites, without policy effect.

use chrono::{DateTime, Duration as ChronoDuration, Utc};

use crate::config::NotifyConfig;
use crate::store::{NewEvent, TriagedMessage};
use crate::triage::DeadlineHit;
use crate::types::{AccountId, Disposition, EventKind, SenderRule, Sensitivity, Tier};

#[derive(Debug, Clone, Copy)]
pub struct EventContext<'a> {
    pub account_id: AccountId,
    pub message_id: i64,
    pub thread_id: &'a str,
    pub sender: &'a str,
    pub one_line: &'a str,
    pub notify_eligible_at: Option<DateTime<Utc>>,
    pub sensitivity: Sensitivity,
    pub is_sent: bool,
    pub is_spam: bool,
    pub rule: Option<Disposition>,
    pub tier: Tier,
    pub importance: u8,
    pub deadline: Option<&'a DeadlineHit>,
}

const MAX_FUTURE_SKEW_SECS: i64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotWorthy,
    Suppressed,
    Expired,
}

pub fn is_fresh(received_at: DateTime<Utc>, cfg: &NotifyConfig, now: DateTime<Utc>) -> bool {
    let floor = now - ChronoDuration::seconds(cfg.freshness_window_secs as i64);
    let ceiling = now + ChronoDuration::seconds(MAX_FUTURE_SKEW_SECS);
    received_at >= floor && received_at <= ceiling
}

pub fn worthy_kind(
    ctx: &EventContext<'_>,
    cfg: &NotifyConfig,
    now: DateTime<Utc>,
) -> Result<EventKind, Refusal> {
    notification_kind(ctx, false, cfg, now)
}

fn notification_kind(
    ctx: &EventContext<'_>,
    is_auth: bool,
    cfg: &NotifyConfig,
    now: DateTime<Utc>,
) -> Result<EventKind, Refusal> {
    if ctx.is_sent || ctx.is_spam {
        return Err(Refusal::NotWorthy);
    }
    let eligible_at = ctx.notify_eligible_at.ok_or(Refusal::NotWorthy)?;
    if !is_auth && ctx.importance < cfg.min_importance {
        return Err(Refusal::NotWorthy);
    }
    if now - eligible_at > ChronoDuration::seconds(cfg.rescue_window_secs as i64) {
        return Err(Refusal::Expired);
    }
    Ok(if is_auth {
        EventKind::Urgent
    } else {
        EventKind::Surfaced
    })
}

pub fn event_for(
    ctx: &EventContext<'_>,
    cfg: &NotifyConfig,
    now: DateTime<Utc>,
) -> Result<NewEvent, Refusal> {
    notification_event(ctx, false, cfg, now)
}

pub fn notification_event(
    ctx: &EventContext<'_>,
    is_auth: bool,
    cfg: &NotifyConfig,
    now: DateTime<Utc>,
) -> Result<NewEvent, Refusal> {
    let kind = notification_kind(ctx, is_auth, cfg, now)?;
    Ok(NewEvent {
        account_id: ctx.account_id,
        message_id: ctx.message_id,
        thread_id: ctx.thread_id.to_string(),
        kind,
        tier: Tier::Signal,
        importance: ctx.importance,
        sender: ctx.sender.to_string(),
        one_line: ctx.one_line.to_string(),
        deadline: None,
        sealed_kind: None,
    })
}

pub fn ingest_context<'a>(
    triaged: &'a TriagedMessage,
    message_id: i64,
    rules: &[SenderRule],
) -> EventContext<'a> {
    let rule = triaged
        .matched_rule
        .and_then(|id| rules.iter().find(|r| r.id == id))
        .map(|r| r.disposition);
    EventContext {
        account_id: triaged.message.account_id,
        message_id,
        thread_id: &triaged.message.thread_id,
        sender: &triaged.message.from_addr,
        one_line: &triaged.one_line,
        notify_eligible_at: triaged.notify_eligible_at,
        sensitivity: triaged.sensitivity,
        is_sent: triaged.message.is_sent,
        is_spam: triaged.message.is_spam,
        rule,
        tier: triaged.tier,
        importance: triaged.importance,
        deadline: triaged.deadline.as_ref(),
    }
}

pub fn seed_context<'a>(
    _row: &'a crate::store::Stage1Queued,
    _seed: &'a crate::store::SeedVerdict,
    _deadline: Option<&'a DeadlineHit>,
    _rule: Option<Disposition>,
) -> Option<EventContext<'a>> {
    None
}

pub fn current_rule(from_addr: &str, rules: &[SenderRule]) -> Option<Disposition> {
    crate::triage::rules::match_sender_rule(from_addr, rules).map(|r| r.disposition)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(now: DateTime<Utc>) -> EventContext<'static> {
        EventContext {
            account_id: 1,
            message_id: 7,
            thread_id: "t1",
            sender: "a@example.com",
            one_line: "A new message",
            notify_eligible_at: Some(now),
            sensitivity: Sensitivity::Normal,
            is_sent: false,
            is_spam: false,
            rule: None,
            tier: Tier::Signal,
            importance: 70,
            deadline: None,
        }
    }

    #[test]
    fn auth_bypasses_score_but_not_arrival_eligibility() {
        let now = Utc::now();
        let cfg = NotifyConfig::default();
        let mut ctx = context(now);
        ctx.importance = 0;
        ctx.rule = Some(Disposition::Squelch);
        ctx.sensitivity = Sensitivity::Sealed;
        let event = notification_event(&ctx, true, &cfg, now).unwrap();
        assert_eq!(event.kind, EventKind::Urgent);
        assert_eq!(event.message_id, ctx.message_id);
        assert_eq!(event.sealed_kind, None);
        ctx.notify_eligible_at = None;
        assert_eq!(
            notification_kind(&ctx, true, &cfg, now),
            Err(Refusal::NotWorthy)
        );
        ctx.notify_eligible_at = Some(now - ChronoDuration::hours(2));
        assert_eq!(
            notification_kind(&ctx, true, &cfg, now),
            Err(Refusal::Expired)
        );
    }

    #[test]
    fn nonauth_uses_only_notification_score() {
        let now = Utc::now();
        let cfg = NotifyConfig::default();
        let mut ctx = context(now);
        ctx.importance = cfg.min_importance - 1;
        ctx.tier = Tier::PastDue;
        assert_eq!(worthy_kind(&ctx, &cfg, now), Err(Refusal::NotWorthy));
        ctx.importance = cfg.min_importance;
        ctx.tier = Tier::Noise;
        ctx.rule = Some(Disposition::Filtered);
        ctx.sensitivity = Sensitivity::Sealed;
        assert_eq!(worthy_kind(&ctx, &cfg, now), Ok(EventKind::Surfaced));
        ctx.is_spam = true;
        assert_eq!(worthy_kind(&ctx, &cfg, now), Err(Refusal::NotWorthy));
        ctx.is_spam = false;
        ctx.is_sent = true;
        assert_eq!(worthy_kind(&ctx, &cfg, now), Err(Refusal::NotWorthy));
    }

    #[test]
    fn eligibility_uses_original_arrival_and_has_inclusive_boundaries() {
        let now = Utc::now();
        let cfg = NotifyConfig::default();
        assert!(is_fresh(
            now - ChronoDuration::seconds(cfg.freshness_window_secs as i64),
            &cfg,
            now
        ));
        assert!(!is_fresh(
            now - ChronoDuration::seconds(cfg.freshness_window_secs as i64 + 1),
            &cfg,
            now
        ));
        assert!(!is_fresh(
            now + ChronoDuration::seconds(MAX_FUTURE_SKEW_SECS + 1),
            &cfg,
            now
        ));
        let mut ctx = context(now);
        ctx.notify_eligible_at = Some(now - ChronoDuration::seconds(cfg.rescue_window_secs as i64));
        assert!(event_for(&ctx, &cfg, now).is_ok());
        assert_eq!(
            worthy_kind(&ctx, &cfg, now + ChronoDuration::seconds(1)),
            Err(Refusal::Expired)
        );
    }
}
