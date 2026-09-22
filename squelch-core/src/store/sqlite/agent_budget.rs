//! Atomic spend reservations for the bounded investigation worker.
use super::*;

pub(super) fn reserve(
    store: &SqliteStore,
    account: AccountId,
    day: &str,
    limits: &[(String, u32)],
) -> Result<bool> {
    let mut connection = store.lock()?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for (key, cap) in limits {
        let used: i64 = transaction.query_row(
            "SELECT COALESCE((SELECT model_calls FROM wake_budget WHERE account_id=?1 AND thread_id=?2 AND day=?3),0)",
            params![account,key,day], |row| row.get(0))?;
        if used >= i64::from(*cap) {
            return Ok(false);
        }
    }
    for (key, _) in limits {
        transaction.execute(
            "INSERT INTO wake_budget(account_id,thread_id,day,model_calls) VALUES(?1,?2,?3,1)
            ON CONFLICT(account_id,thread_id,day) DO UPDATE SET model_calls=model_calls+1",
            params![account, key, day],
        )?;
    }
    transaction.commit()?;
    Ok(true)
}

pub(super) fn refund(
    store: &SqliteStore,
    account: AccountId,
    day: &str,
    limits: &[(String, u32)],
) -> Result<()> {
    let mut connection = store.lock()?;
    let transaction = connection.transaction()?;
    for (key, _) in limits {
        transaction.execute("UPDATE wake_budget SET model_calls=MAX(0,model_calls-1) WHERE account_id=?1 AND thread_id=?2 AND day=?3",
            params![account,key,day])?;
    }
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::agent_triage::AgentTriageStore;

    #[test]
    fn scoped_budget_is_atomic_and_refundable_on_the_original_day() {
        let store = SqliteStore::open_in_memory().unwrap();
        let account = store.ensure_account("budget@example.com").unwrap();
        let limits = vec![("global".into(), 2), ("thread:a".into(), 1)];
        assert!(
            store
                .reserve_agent_budget(account, "2026-09-17", &limits)
                .unwrap()
        );
        assert!(
            !store
                .reserve_agent_budget(account, "2026-09-17", &limits)
                .unwrap()
        );
        assert_eq!(
            store
                .stage2_budget_used(account, "global", "2026-09-17")
                .unwrap(),
            1
        );
        store
            .refund_agent_budget(account, "2026-09-17", &limits)
            .unwrap();
        assert!(
            store
                .reserve_agent_budget(account, "2026-09-17", &limits)
                .unwrap()
        );
        assert!(
            store
                .reserve_agent_budget(account, "2026-09-18", &limits)
                .unwrap()
        );
        store
            .refund_agent_budget(account, "2026-09-17", &limits)
            .unwrap();
        assert_eq!(
            store
                .stage2_budget_used(account, "global", "2026-09-18")
                .unwrap(),
            1
        );
    }

    #[test]
    fn concurrent_threads_cannot_overshoot_the_account_cap() {
        let store = std::sync::Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("concurrent@example.com").unwrap();
        let handles: Vec<_> = (0..16)
            .map(|index| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store
                        .reserve_agent_budget(
                            account,
                            "today",
                            &[("global".into(), 3), (format!("thread:{index}"), 1)],
                        )
                        .unwrap()
                })
            })
            .collect();
        let won = handles
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum::<usize>();
        assert_eq!(won, 3);
        assert_eq!(
            store
                .stage2_budget_used(account, "global", "today")
                .unwrap(),
            3
        );
    }
}
