//! Carrier polling projections of explicit agent delivery facts.
//!
//! Canonical decisions own these rows. Retraction rebuilds from remaining
//! proposals and removes an agent-created row when its last proposal disappears.
use super::*;
use crate::triage::decision::{MessageDecision, RecordProposal};
use crate::triage::extract::shipments::sanitize_tracking_number;
use std::collections::BTreeSet;

fn tracking_numbers(decision: &MessageDecision) -> impl Iterator<Item = String> {
    decision.records.iter().filter_map(|record| match record {
        RecordProposal::Delivery {
            tracking_number: Some(number),
            ..
        } => sanitize_tracking_number(Some(number), None),
        _ => None,
    })
}

pub(super) fn reconcile(
    conn: &Connection,
    account: AccountId,
    previous: Option<&MessageDecision>,
    current: &MessageDecision,
) -> Result<()> {
    let affected: BTreeSet<String> = previous
        .into_iter()
        .flat_map(tracking_numbers)
        .chain(tracking_numbers(current))
        .collect();
    if affected.is_empty() {
        return Ok(());
    }
    let mut statement = conn.prepare(
        "SELECT d.message_id,d.decision_json,m.received_at
         FROM agent_message_decisions d JOIN messages m ON m.id=d.message_id AND m.account_id=d.account_id
         WHERE d.account_id=?1 AND m.is_sent=0 AND m.is_spam=0 ORDER BY m.received_at DESC,m.id DESC",
    )?;
    let decisions = statement
        .query_map([account], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, dt(row, 2)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut proposals = Vec::new();
    for (message, encoded, received) in decisions {
        let decision: MessageDecision =
            serde_json::from_str(&encoded).map_err(|e| CoreError::Other(e.into()))?;
        for record in decision.records {
            if let RecordProposal::Delivery {
                carrier: Some(carrier),
                tracking_number: Some(number),
                status,
                ..
            } = record
                && let Some(number) = sanitize_tracking_number(Some(&number), None)
                && affected.contains(&number)
                && matches!(
                    carrier.as_str(),
                    "ups" | "usps" | "fedex" | "dhl" | "amazon"
                )
                && let Some(status) = crate::triage::ShipmentStatus::parse(&status)
            {
                proposals.push((
                    message,
                    received,
                    crate::triage::ShipmentInfo {
                        tracking_url: crate::triage::shipment::tracking_url(&carrier, &number),
                        carrier,
                        tracking_number: number,
                        status,
                        item_name: String::new(),
                    },
                ));
            }
        }
    }
    for number in affected {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM shipments WHERE account_id=?1 AND tracking_number=?2",
                params![account, number],
                |row| row.get(0),
            )
            .optional()?;
        let latest = proposals
            .iter()
            .find(|(_, _, info)| info.tracking_number == number);
        let Some((message, received, info)) = latest else {
            if let Some(id) = existing {
                // Imported legacy rows belong to their original subsystem and
                // remain intact; only rows created by this projection are retired.
                conn.execute("DELETE FROM shipments WHERE account_id=?1 AND id=?2 AND EXISTS(
                    SELECT 1 FROM agent_delivery_projections p WHERE p.account_id=?1 AND p.shipment_id=?2)", params![account,id])?;
                conn.execute(
                    "DELETE FROM agent_delivery_projections WHERE account_id=?1 AND shipment_id=?2",
                    params![account, id],
                )?;
            }
            continue;
        };
        let carrier_status: Option<String> = conn.query_row(
            "SELECT status FROM shipments WHERE account_id=?1 AND tracking_number=?2 AND carrier_status_raw IS NOT NULL",
            params![account,number], |row|row.get(0),
        ).optional()?;
        let id = if let Some(id) = existing {
            id
        } else {
            // Reuse insertion and provenance plumbing only. Existing rows do
            // not pass through the legacy name or monotonic-status merge.
            let id =
                super::specialists::upsert_shipment_conn(conn, account, *message, info, *received)?;
            conn.execute(
                "INSERT INTO agent_delivery_projections(account_id,shipment_id) VALUES(?1,?2)",
                params![account, id],
            )?;
            id
        };
        let managed: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_delivery_projections WHERE account_id=?1 AND shipment_id=?2)",params![account,id],|r|r.get(0))?;
        if managed {
            // The newest retained explicit proposal is authoritative for email
            // fields. Carrier observations remain authoritative once polled.
            conn.execute("UPDATE shipments SET carrier=?3,created_by_message_id=?4,last_message_id=?4,
                 tracking_url=?7,last_update=MAX(last_update,?8),
                 status=COALESCE(?6,?5),
                 delivered_at=CASE WHEN carrier_status_raw IS NULL AND ?5!='delivered' THEN NULL ELSE delivered_at END
                 WHERE account_id=?1 AND id=?2",params![account,id,info.carrier,message,info.status.as_str(),carrier_status,info.tracking_url,received.to_rfc3339()])?;
        } else {
            // A LEGACY row is never re-identified or retired here, but newer
            // mail about its package is still news, and nothing else is left to
            // record it: take the status and move `last_update`, which is what
            // returns a row the listing hid as silent. Only mail NEWER than the
            // row counts, so re-triaging old mail rewrites nothing.
            conn.execute("UPDATE shipments SET last_message_id=?3,last_update=?6,
                 status=COALESCE(?5,?4),
                 delivered_at=CASE WHEN COALESCE(?5,?4)='delivered' THEN COALESCE(delivered_at,?6) ELSE delivered_at END
                 WHERE account_id=?1 AND id=?2 AND last_update<?6",params![account,id,message,info.status.as_str(),carrier_status,received.to_rfc3339()])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> SqliteStore {
        let store = SqliteStore::open_in_memory().unwrap();
        let conn = store.lock().unwrap();
        conn.execute(
            "INSERT INTO accounts(id,email,created_at) VALUES(1,'me@test',?1)",
            [Utc::now().to_rfc3339()],
        )
        .unwrap();
        for id in [1, 2] {
            conn.execute(
                "INSERT INTO messages(id,account_id,gmail_msg_id,thread_id,from_addr,subject,received_at,snippet,body)
                 VALUES(?1,1,?2,?2,'shipping@test','Package',?3,'','')",
                params![id,id.to_string(),(Utc::now()-chrono::Duration::hours(3-id)).to_rfc3339()],
            ).unwrap();
        }
        drop(conn);
        store
    }

    fn delivery(number: &str, status: &str) -> MessageDecision {
        MessageDecision {
            records: vec![RecordProposal::Delivery {
                carrier: Some("ups".into()),
                tracking_number: Some(number.into()),
                status: status.into(),
                evidence: vec![],
            }],
            ..Default::default()
        }
    }

    fn write(
        store: &SqliteStore,
        id: i64,
        previous: Option<&MessageDecision>,
        current: &MessageDecision,
    ) {
        let mut conn = store.lock().unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "INSERT INTO agent_message_decisions(account_id,message_id,revision,decision_json,decided_at)
             VALUES(1,?1,1,?2,?3) ON CONFLICT(account_id,message_id) DO UPDATE SET decision_json=excluded.decision_json",
            params![id,serde_json::to_string(current).unwrap(),Utc::now().to_rfc3339()],
        ).unwrap();
        reconcile(&tx, 1, previous, current).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn normalized_tracking_coalesces_polling_and_retracts_across_source_spellings() {
        let store = fixture();
        let spaced = delivery(" 1z999 aa10 1234 56784 ", "shipped");
        write(&store, 1, None, &spaced);
        let pollable = store
            .list_pollable_shipments(1, Utc::now() - chrono::Duration::days(1), 5)
            .unwrap();
        assert_eq!(pollable.len(), 1);
        assert_eq!(pollable[0].tracking_number, "1Z999AA10123456784");
        assert!(
            pollable[0]
                .tracking_url
                .as_deref()
                .unwrap()
                .ends_with("1Z999AA10123456784")
        );
        let compact = delivery("1Z999AA10123456784", "out_for_delivery");
        write(&store, 2, None, &compact);
        let rows = store
            .list_pollable_shipments(1, Utc::now() - chrono::Duration::days(1), 5)
            .unwrap();
        assert_eq!(
            rows.len(),
            1,
            "equivalent spellings identify one polling row"
        );
        assert_eq!(rows[0].id, pollable[0].id);
        assert_eq!(rows[0].status, "out_for_delivery");
        write(&store, 2, Some(&compact), &MessageDecision::default());
        let remaining = store
            .list_pollable_shipments(1, Utc::now() - chrono::Duration::days(1), 5)
            .unwrap();
        assert_eq!(
            remaining.len(),
            1,
            "spaced source remains after compact source retracts"
        );
        assert_eq!(remaining[0].status, "shipped");
        write(&store, 1, Some(&spaced), &MessageDecision::default());
        assert!(
            store
                .list_pollable_shipments(1, Utc::now() - chrono::Duration::days(1), 5)
                .unwrap()
                .is_empty()
        );
        let marker_count: i64 = store
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM agent_delivery_projections", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            marker_count, 0,
            "normalized final retraction removes ownership marker"
        );
    }

    /// A row written before the agent owned deliveries, `age_days` silent.
    fn legacy_row(store: &SqliteStore, number: &str, age_days: i64) -> i64 {
        let at = (Utc::now() - chrono::Duration::days(age_days)).to_rfc3339();
        let conn = store.lock().unwrap();
        conn.execute(
            "INSERT INTO shipments(account_id,tracking_number,carrier,item_name,status,first_seen,last_update)
             VALUES(1,?1,'ups','Lamp','shipped',?2,?2)",
            params![number, at],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn listed(store: &SqliteStore) -> Vec<crate::types::Shipment> {
        let policy = crate::config::ShipmentListPolicy::default();
        assert!(
            policy.stale_after_days > 0,
            "the default policy goes silent"
        );
        store.list_shipments(1, false, policy).unwrap()
    }

    /// THE PRODUCTION REVIVAL PATH. `reconcile` is the only mail-driven writer
    /// left, so a row the listing hid as silent comes back through here or not
    /// at all, and a legacy row is in no projection to be updated through.
    #[test]
    fn newer_mail_returns_a_silent_legacy_row_without_adopting_it() {
        let store = fixture();
        let id = legacy_row(&store, "1Z999AA10123456784", 30);
        assert!(listed(&store).is_empty(), "silent for 30 days: hidden");

        let update = delivery("1Z999AA10123456784", "out_for_delivery");
        write(&store, 2, None, &update);
        let rows = listed(&store);
        assert_eq!(rows.len(), 1, "the update email brought it back");
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].status, "out_for_delivery");
        assert_eq!(rows[0].item_name, "Lamp", "the legacy name is untouched");

        // Retracting that mail must not delete a row this projection never owned.
        write(&store, 2, Some(&update), &MessageDecision::default());
        let kept: i64 = store
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM shipments WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kept, 1, "legacy rows are never retired from here");
    }

    /// Re-triaging OLD mail is not news: it must not resurrect the row, or a
    /// re-triage pass would refill the list with every dead package at once.
    #[test]
    fn re_deciding_old_mail_does_not_return_a_silent_legacy_row() {
        let store = fixture();
        legacy_row(&store, "1Z999AA10123456784", 30);
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE messages SET received_at=?1 WHERE id=1",
                [(Utc::now() - chrono::Duration::days(40)).to_rfc3339()],
            )
            .unwrap();
        write(
            &store,
            1,
            None,
            &delivery("1Z999AA10123456784", "delivered"),
        );
        assert!(listed(&store).is_empty());
        let status: String = store
            .lock()
            .unwrap()
            .query_row("SELECT status FROM shipments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            status, "shipped",
            "mail older than the row rewrites nothing"
        );
    }

    /// The same revival for a row this projection DOES own.
    #[test]
    fn newer_mail_returns_a_silent_managed_row() {
        let store = fixture();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE messages SET received_at=?1 WHERE id=1",
                [(Utc::now() - chrono::Duration::days(30)).to_rfc3339()],
            )
            .unwrap();
        write(&store, 1, None, &delivery("1Z999AA10123456784", "shipped"));
        assert!(
            listed(&store).is_empty(),
            "minted from 30-day-old mail: hidden"
        );
        write(
            &store,
            2,
            None,
            &delivery("1Z999AA10123456784", "out_for_delivery"),
        );
        assert_eq!(listed(&store).len(), 1);
    }

    #[test]
    fn invalid_tracking_stays_a_canonical_fact_without_entering_carrier_polling() {
        let store = fixture();
        for (id, number) in [(1, "https://example.test/order/1234567890"), (2, "1234")] {
            write(&store, id, None, &delivery(number, "shipped"));
        }
        let conn = store.lock().unwrap();
        let records: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_message_decisions", [], |r| {
                r.get(0)
            })
            .unwrap();
        let shipments: i64 = conn
            .query_row("SELECT COUNT(*) FROM shipments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(records, 2);
        assert_eq!(shipments, 0);
    }
}
