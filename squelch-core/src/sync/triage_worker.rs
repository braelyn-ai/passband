//! Durable model work. Ingest never waits for these decisions.
use super::*;
use crate::metrics::AgentVerdict;
use crate::store::agent_triage::{
    AgentCommitOutcome, AgentContext, AgentJob, AgentMessage, AgentSourceSnapshot,
};
use crate::triage::agent::{AgentConnection, run_agent};
use crate::triage::context::{ContextSnapshot, EvidenceReader, EvidenceRequest, EvidenceResult};
use std::collections::BTreeMap;

struct StoreEvidence<'a, S: Store> {
    store: &'a S,
    account: AccountId,
    limit: usize,
    sources: std::sync::Mutex<Vec<AgentSourceSnapshot>>,
}

fn read_job(account: AccountId, message_id: i64) -> AgentJob {
    AgentJob {
        id: 0,
        account_id: account,
        message_id,
        kind: "triage".into(),
        trigger: "context".into(),
        lease_token: String::new(),
        attempts: 0,
        arrival_eligible: false,
    }
}

impl<S: Store> EvidenceReader for StoreEvidence<'_, S> {
    fn read(&self, request: &EvidenceRequest) -> std::result::Result<EvidenceResult, String> {
        let result = self
            .read_inner(request)
            .map_err(|_| "evidence_unavailable".to_owned())?;
        Ok(result)
    }
}

impl<S: Store> StoreEvidence<'_, S> {
    fn read_inner(&self, request: &EvidenceRequest) -> Result<EvidenceResult> {
        let mut revisions = BTreeMap::new();
        let messages = match request {
            EvidenceRequest::ReadMessage { message_id, offset } => {
                self.store
                    .snapshot_agent_sources(self.account, &[*message_id])?;
                let context = self
                    .store
                    .load_agent_context(&read_job(self.account, *message_id))?;
                let message = &context.message;
                self.sources
                    .lock()
                    .expect("evidence sources")
                    .push(message.source.clone());
                revisions.insert(
                    message.thread_id.clone(),
                    context.revision.attention_revision,
                );
                let offset = offset.unwrap_or(0);
                // Character offsets keep Unicode boundaries stable across pages.
                let page: String = message.body.chars().skip(offset).take(2000).collect();
                let next = offset.saturating_add(page.chars().count());
                let more = message.body.chars().count() > next;
                let mut value = serde_json::to_value(message)
                    .map_err(|_| CoreError::InvalidInput("message serialization".into()))?;
                value["body"] = page.into();
                value["body_truncated"] = more.into();
                value["body_offset"] = offset.into();
                value["next_offset"] = if more {
                    next.into()
                } else {
                    serde_json::Value::Null
                };
                return Ok(EvidenceResult {
                    data: serde_json::json!({"message": value}),
                    source_message_ids: vec![*message_id],
                    thread_revisions: revisions,
                });
            }
            EvidenceRequest::ReadThread { thread_id } => {
                let context = self.store.agent_thread_context(self.account, thread_id)?;
                // Attention and its message snapshots must describe one atomic
                // read; mixing a previous decision with newer source revisions
                // would let stale evidence pass commit validation.
                let messages = context
                    .thread
                    .iter()
                    .take(self.limit)
                    .cloned()
                    .collect::<Vec<_>>();
                self.sources
                    .lock()
                    .expect("evidence sources")
                    .extend(messages.iter().map(|m| m.source.clone()));
                revisions.insert(thread_id.clone(), context.revision.attention_revision);
                return Ok(EvidenceResult {
                    source_message_ids: messages.iter().map(|m| m.id).collect(),
                    data: serde_json::json!({"messages": messages, "attention": context.attention,
                        "user_corrections": context.corrections, "previous_decision": context.previous_decision}),
                    thread_revisions: revisions,
                });
            }
            EvidenceRequest::SearchMail { query } => {
                self.store
                    .agent_search_mail(self.account, query, self.limit)?
            }
            EvidenceRequest::ReadSenderHistory { sender } => {
                self.store
                    .agent_sender_history(self.account, sender, self.limit)?
            }
            EvidenceRequest::ReadRecord { message_id } => {
                self.store
                    .snapshot_agent_sources(self.account, &[*message_id])?;
                let context = self
                    .store
                    .load_agent_context(&read_job(self.account, *message_id))?;
                self.sources
                    .lock()
                    .expect("evidence sources")
                    .push(context.message.source.clone());
                return Ok(EvidenceResult {
                    data: serde_json::json!({"records": context.previous_decision.map(|d| d.records)}),
                    source_message_ids: vec![*message_id],
                    thread_revisions: BTreeMap::new(),
                });
            }
            EvidenceRequest::ReadAttachment {
                message_id,
                attachment_id,
            } => {
                let message = self.store.agent_read_message(self.account, *message_id)?;
                self.sources
                    .lock()
                    .expect("evidence sources")
                    .push(message.source.clone());
                let thread = self
                    .store
                    .thread_view_with_html(self.account, &message.thread_id)?;
                let id = attachment_id
                    .parse::<i64>()
                    .map_err(|_| CoreError::NotFound)?;
                let belongs = thread
                    .messages
                    .iter()
                    .filter(|m| m.id == *message_id)
                    .flat_map(|m| &m.attachments)
                    .any(|a| a.id == id);
                if !belongs {
                    return Err(CoreError::NotFound);
                }
                let (_, mime, bytes) = self
                    .store
                    .attachment_bytes(self.account, id)?
                    .ok_or(CoreError::NotFound)?;
                let data = match bytes {
                    Some(bytes) if mime.starts_with("text/") || mime == "application/json" => {
                        let text = String::from_utf8(bytes).map_err(|_| {
                            CoreError::InvalidInput("unsupported attachment encoding".into())
                        })?;
                        serde_json::json!({"text": crate::text::truncate_chars(&text, 16_000), "truncated": text.chars().count() > 16_000})
                    }
                    Some(_) => serde_json::json!({"unsupported": true, "mime": mime}),
                    None => {
                        serde_json::json!({"unavailable": true, "reason": "attachment bytes not stored"})
                    }
                };
                return Ok(EvidenceResult {
                    data,
                    source_message_ids: vec![*message_id],
                    thread_revisions: BTreeMap::new(),
                });
            }
        };
        if let Some(message) = messages.first() {
            let context = self
                .store
                .load_agent_context(&read_job(self.account, message.id))?;
            revisions.insert(
                message.thread_id.clone(),
                context.revision.attention_revision,
            );
        }
        self.sources
            .lock()
            .expect("evidence sources")
            .extend(messages.iter().map(|m| m.source.clone()));
        let sources = messages.iter().map(|m| m.id).collect();
        Ok(EvidenceResult {
            data: serde_json::json!({"messages": messages}),
            source_message_ids: sources,
            thread_revisions: revisions,
        })
    }
}

fn bounded_message(message: &AgentMessage, chars: usize) -> serde_json::Value {
    let mut value = serde_json::to_value(message).expect("message serialization");
    value["body"] = crate::text::truncate_chars(&message.body, chars).into();
    value["body_truncated"] = (message.body.chars().count() > chars).into();
    value
}

fn snapshot(context: &AgentContext, job: &AgentJob, limit: usize) -> ContextSnapshot {
    let siblings = context.thread.iter().take(limit).collect::<Vec<_>>();
    let mut sources = siblings.iter().map(|m| m.id).collect::<Vec<_>>();
    sources.push(context.message.id);
    sources.sort_unstable();
    sources.dedup();
    ContextSnapshot {
        account_id: job.account_id,
        message_id: job.message_id,
        thread_id: context.message.thread_id.clone(),
        initial: serde_json::json!({
            "message": bounded_message(&context.message, 24_000),
            "thread": siblings.iter().map(|m| bounded_message(m, 6_000)).collect::<Vec<_>>(),
            "previous_decision": context.previous_decision,
            "attention": context.attention,
            "sender_preferences": context.rules,
            "user_corrections": context.corrections,
            "trigger": job.trigger,
            "access_only": context.message.is_sent || context.message.is_spam,
            "now": Utc::now(),
        }),
        source_message_ids: sources,
        thread_revisions: [(
            context.message.thread_id.clone(),
            context.revision.attention_revision,
        )]
        .into_iter()
        .collect(),
        rule_ids: context
            .rules
            .iter()
            .filter_map(|r| r.get("id").and_then(|id| id.as_i64()))
            .collect(),
        memory: Vec::new(),
    }
}

impl<S: Store + 'static, C: CredentialStore + 'static + ?Sized> SyncEngine<S, C> {
    pub(super) async fn agent_triage_pass(&self) {
        if Utc::now().timestamp() < self.agent_retry_after.load(Ordering::Relaxed) {
            return;
        }
        let Some(llm) = &self.stage2_llm else {
            return;
        };
        if let Err(error) = self.config.triage.validate() {
            eprintln!("squelch: invalid triage configuration: {error}");
            return;
        }
        let day = Utc::now().format("%Y-%m-%d").to_string();
        let since = Utc::now() - ChronoDuration::days(self.config.sync.backfill_days as i64);
        if let Err(error) = self.store.initialize_agent_cutover(self.account_id, since) {
            eprintln!("squelch: triage cutover could not queue historical work: {error}");
            return;
        }
        let concurrency = self.config.triage.agent.concurrency;
        let mut jobs = Vec::new();
        let mut revisits = 0;
        for _ in 0..concurrency {
            let kind =
                if self.config.revisit.enabled && revisits < self.config.revisit.batch_per_cycle {
                    "investigation"
                } else {
                    "initial_investigation"
                };
            match self.store.claim_agent_job(
                self.account_id,
                kind,
                Utc::now(),
                self.config.triage.agent.timeout_secs as i64 + 30,
            ) {
                Ok(Some(job)) => {
                    if job.trigger.starts_with("revisit:") {
                        revisits += 1;
                    }
                    jobs.push(job);
                }
                Ok(None) => break,
                Err(error) => {
                    eprintln!("squelch: triage claim failed: {error}");
                    break;
                }
            }
        }
        futures::future::join_all(
            jobs.into_iter()
                .map(|job| self.process_agent_job(job, llm, &day)),
        )
        .await;
    }

    pub(super) async fn process_agent_job(&self, job: AgentJob, llm: &ResolvedLlm, day: &str) {
        let mut context = match self.store.load_agent_context(&job) {
            Ok(context) => context,
            Err(_) => {
                self.retry_agent(&job, "context_unavailable");
                return;
            }
        };
        let now = Utc::now();
        let retry_after = self.agent_retry_after.load(Ordering::Relaxed);
        if now.timestamp() < retry_after {
            let until = DateTime::from_timestamp(retry_after, 0).unwrap_or(now);
            let _ = self.store.defer_agent_job(&job, until, "provider_cooldown");
            return;
        }
        if job.trigger.starts_with("revisit:") && !self.config.revisit.enabled {
            let _ = self.store.defer_agent_job(
                &job,
                now + ChronoDuration::hours(1),
                "revisits_disabled",
            );
            return;
        }
        let limits = match self.investigation_budget_limits(&job, &context) {
            Ok(limits) => limits,
            Err(_) => {
                self.retry_agent(&job, "budget_unavailable");
                return;
            }
        };
        match self
            .store
            .reserve_agent_budget(self.account_id, day, &limits)
        {
            Ok(true) => {}
            Ok(false) => {
                let tomorrow = (now.date_naive() + ChronoDuration::days(1))
                    .and_hms_opt(0, 0, 1)
                    .expect("valid midnight")
                    .and_utc();
                let _ = self
                    .store
                    .defer_agent_job(&job, tomorrow, "daily_budget_exhausted");
                self.metrics.record_agent(AgentVerdict::Deferred);
                return;
            }
            Err(_) => {
                self.retry_agent(&job, "budget_unavailable");
                return;
            }
        }
        let mut exposed_ids = context
            .thread
            .iter()
            .take(self.config.triage.context.initial_thread_messages)
            .map(|message| message.id)
            .collect::<Vec<_>>();
        exposed_ids.push(context.message.id);
        exposed_ids.sort_unstable();
        exposed_ids.dedup();
        if self
            .store
            .snapshot_agent_sources(self.account_id, &exposed_ids)
            .is_err()
        {
            let _ = self
                .store
                .refund_agent_budget(self.account_id, day, &limits);
            self.retry_agent(&job, "context_unavailable");
            return;
        }
        // Snapshot the content shown in this investigation, not unseen siblings.
        // Initialization above ensures observed historical sources have an access
        // job; commit still compares the original evidence versions against now.
        let reader = StoreEvidence {
            store: self.store.as_ref(),
            account: self.account_id,
            limit: self.config.triage.context.max_related_messages,
            sources: std::sync::Mutex::new(
                exposed_ids
                    .iter()
                    .filter_map(|id| {
                        context
                            .thread
                            .iter()
                            .chain(std::iter::once(&context.message))
                            .find(|m| m.id == *id)
                            .map(|m| m.source.clone())
                    })
                    .collect(),
            ),
        };
        let mut config = self.config.triage.clone();
        let base_model = config
            .agent
            .model
            .as_deref()
            .unwrap_or(&self.config.stage2.model)
            .to_string();
        if llm.provider == Stage2Provider::Anthropic && crate::triage::llm::is_gateway_url(&llm.url)
        {
            config.agent.model = Some(
                crate::triage::llm::qualify_gateway_model(&base_model)
                    .unwrap_or(base_model.clone()),
            );
            if let Some(review) = &config.agent.review_model {
                config.agent.review_model = Some(
                    crate::triage::llm::qualify_gateway_model(review).unwrap_or(review.clone()),
                );
            }
        }
        let input = snapshot(&context, &job, config.context.initial_thread_messages);
        let connection = AgentConnection {
            http: &self.http,
            url: &llm.url,
            api_key: &llm.api_key,
            provider: llm.provider,
            model: &base_model,
            effort: self.config.stage2.effort.as_deref(),
        };
        match run_agent(connection, &config, input, &reader).await {
            Ok(run) => {
                context.run_metadata = serde_json::json!({
                    "prompt_version": crate::triage::agent::PROMPT_VERSION,
                    "schema_version": 1,
                    "model": config.agent.model.as_deref().unwrap_or(&base_model),
                    "review_model": config.agent.review_model,
                    "config": config,
                    "model_turns": run.model_turns,
                    "tool_calls": run.tool_calls,
                    "source_message_ids": run.source_message_ids,
                    "usage": run.usage,
                });
                for usage in run.usage {
                    let _ = self
                        .store
                        .stage2_bump_usage(self.account_id, day, usage.into());
                }
                let sources = reader.sources.lock().expect("evidence sources").clone();
                match self.store.commit_agent_decision_with_policy(
                    &job,
                    &context,
                    &run.decision,
                    &sources,
                    &self.config.revisit,
                ) {
                    // The commit transaction queues a durable deliberate notification job.
                    Ok(AgentCommitOutcome::Applied) => {
                        self.metrics.record_agent(AgentVerdict::Applied);
                    }
                    Ok(AgentCommitOutcome::Stale) => {
                        self.metrics.record_agent(AgentVerdict::Stale);
                        self.retry_agent(&job, "context_changed");
                    }
                    Err(_) => {
                        self.retry_agent(&job, "commit_failed");
                    }
                }
            }
            Err(error) => {
                for usage in error.usage {
                    let _ = self
                        .store
                        .stage2_bump_usage(self.account_id, day, usage.into());
                }
                if is_provider_outage(&error.kind) {
                    if crate::triage::llm::is_config_failure(&error.kind) {
                        self.metrics.record_llm_config_failure();
                        // Only a first-call rejection guarantees this reservation
                        // bought no accepted model work. Paid prior turns stay charged.
                        if error.model_calls <= 1 {
                            let _ = self
                                .store
                                .refund_agent_budget(self.account_id, day, &limits);
                        }
                    }
                    let until = Utc::now()
                        + ChronoDuration::seconds(
                            self.config
                                .triage
                                .agent
                                .outage_retry_secs
                                .min(i64::MAX as u64) as i64,
                        );
                    self.agent_retry_after
                        .fetch_max(until.timestamp(), Ordering::Relaxed);
                    let _ = self.store.defer_agent_job(&job, until, &error.kind);
                    self.metrics.record_agent(AgentVerdict::Retryable);
                } else {
                    self.retry_agent(&job, &error.kind);
                }
            }
        }
    }

    fn investigation_budget_limits(
        &self,
        job: &AgentJob,
        context: &AgentContext,
    ) -> Result<Vec<(String, u32)>> {
        let overrides = self.store.stage2_cap_overrides(self.account_id)?;
        let global = self
            .config
            .triage
            .agent
            .daily_run_cap
            .min(
                overrides
                    .global_daily_cap
                    .unwrap_or(self.config.stage2.global_daily_cap),
            )
            .min(
                overrides
                    .stage1_global_daily_cap
                    .unwrap_or(self.config.stage1.global_daily_cap),
            );
        let mut limits = vec![
            (GLOBAL_BUDGET_KEY.into(), global),
            (
                format!("agent_thread:{}", context.message.thread_id),
                overrides
                    .thread_daily_cap
                    .unwrap_or(self.config.stage2.thread_daily_cap),
            ),
            (
                format!(
                    "agent_sender:{}",
                    context.message.from_addr.to_ascii_lowercase()
                ),
                overrides
                    .sender_daily_cap
                    .unwrap_or(self.config.stage2.sender_daily_cap),
            ),
        ];
        if job.trigger.starts_with("revisit:") {
            limits.push(("__agent_revisit__".into(), self.config.revisit.daily_cap));
        }
        Ok(limits)
    }

    fn retry_agent(&self, job: &AgentJob, code: &str) {
        if job.attempts >= i64::from(self.config.triage.agent.max_attempts) {
            self.metrics.record_agent(AgentVerdict::Failed);
            if let Err(error) = self.store.fail_agent_job(job, code) {
                eprintln!("squelch: could not record failed triage job: {error}");
            }
            return;
        }
        self.metrics.record_agent(AgentVerdict::Retryable);
        let seconds = 15_i64
            .saturating_mul(2_i64.pow(job.attempts.clamp(0, 8) as u32))
            .min(3600);
        if let Err(error) =
            self.store
                .retry_agent_job(job, Utc::now() + ChronoDuration::seconds(seconds), code)
        {
            eprintln!("squelch: could not schedule triage retry: {error}");
        }
    }

    pub(super) async fn agent_notification_lane(
        &self,
        shutdown: &mut tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            if *shutdown.borrow() {
                return;
            }
            let mut jobs = Vec::new();
            for (kind, lease_secs) in [
                (
                    "notification",
                    self.config.notify.fast_timeout_secs as i64 + 30,
                ),
                ("deliberate_notification", 30),
            ] {
                for _ in 0..self.config.notify.fast_concurrency.max(1) {
                    match self
                        .store
                        .claim_agent_job(self.account_id, kind, Utc::now(), lease_secs)
                    {
                        Ok(Some(job)) => jobs.push(job),
                        Ok(None) => break,
                        Err(error) => {
                            eprintln!("squelch: notification claim failed: {error}");
                            break;
                        }
                    }
                }
            }
            futures::future::join_all(jobs.into_iter().map(|job| async move {
                let context = match self.store.load_agent_context(&job) {
                    Ok(context) => context,
                    Err(_) => {
                        self.retry_agent(&job, "notify_context_unavailable");
                        return;
                    }
                };
                let result = if job.kind == "deliberate_notification" {
                    match &context.previous_decision {
                        Some(decision) => self.notify_lane().request_assessed(
                            &context,
                            decision.auth.is_auth(),
                            &decision.notification,
                            self.config
                                .triage
                                .agent
                                .model
                                .as_deref()
                                .unwrap_or(&self.config.stage2.model),
                        ),
                        None => {
                            self.retry_agent(&job, "notify_decision_unavailable");
                            return;
                        }
                    }
                } else if let Some(candidate) =
                    notify_lane::candidate_from_context(&context, &self.config.notify)
                {
                    self.notify_lane().clone().run(candidate).await
                } else {
                    Ok(())
                };
                match result {
                    Ok(()) => {
                        if self.store.complete_agent_job(&job).is_err() {
                            self.retry_agent(&job, "notify_completion_unavailable");
                        }
                    }
                    Err(_) => self.retry_agent(&job, "notify_store_unavailable"),
                }
            }))
            .await;
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(self.config.triage.agent.worker_poll_secs)) => {},
                _ = shutdown.changed() => { if *shutdown.borrow() { return; } }
            }
        }
    }
}

/// Provider failures describe shared availability, not a permanently bad email.
fn is_provider_outage(kind: &str) -> bool {
    crate::triage::llm::is_config_failure(kind)
        || matches!(kind, "transport" | "agent_timeout" | "http_429")
        || kind
            .strip_prefix("http_")
            .and_then(|v| v.split(':').next())
            .and_then(|v| v.parse::<u16>().ok())
            .is_some_and(|status| (500..600).contains(&status))
}
