//! Carrier polling projections of explicit agent delivery facts.
//!
//! Canonical decisions own these rows. Retraction rebuilds from remaining
//! proposals and removes an agent-created row when its last proposal disappears.
use super::*;
use crate::triage::ShipmentStatus;
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

/// Write what the newest retained mail about a package says onto its row. ONE
/// place, for owned and legacy rows alike, so there is one answer to "what does
/// newer mail do to a shipment":
///
/// * it becomes the row's click target and its carrier (the mail is the newer
///   witness; if it names FedEx for a number USPS rejected five times, the
///   un-retired poll must go to FedEx, not back to USPS);
/// * it moves `last_update` forward, which is what returns a row the listing
///   hid as silent; a re-decided OLD mail moves nothing;
/// * mail NEWER THAN THE ROW un-retires it. A carrier that said "never heard
///   of it" five times was answering about a label the shipper had not handed
///   over, and fresh mail is the evidence the number is real now. Nothing else
///   can reset the counter, because a retired row is not polled.
///
/// `status` is decided by the caller (see `reconcile`). `created_by_message_id`
/// moves only on an owned row: it is the projection's own provenance, and a
/// legacy row keeps the mail that minted it.
#[allow(clippy::too_many_arguments)]
fn record_newer_mail(
    conn: &Connection,
    account: AccountId,
    id: i64,
    managed: bool,
    message: i64,
    info: &crate::triage::ShipmentInfo,
    status: &str,
    received: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE shipments SET
             carrier=?3,
             tracking_url=?4,
             created_by_message_id=CASE WHEN ?9 THEN ?5 ELSE created_by_message_id END,
             last_message_id=?5,
             poll_failures=CASE WHEN last_update<?7 THEN 0 ELSE poll_failures END,
             last_update=MAX(last_update,?7),
             status=?6,
             delivered_at=CASE
                 WHEN ?6='delivered' THEN COALESCE(delivered_at,?7)
                 WHEN carrier_status_raw IS NULL AND ?9 THEN NULL
                 ELSE delivered_at END
         WHERE account_id=?1 AND id=?2 AND (?8 OR last_update<?7)",
        params![
            account,
            id,
            info.carrier,
            info.tracking_url,
            message,
            status,
            received.to_rfc3339(),
            managed,
            managed,
        ],
    )?;
    Ok(())
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
        // WHAT THE MAIL SAYS THE STATUS IS differs by ownership, and it is the
        // only thing that does. An OWNED row is a projection of the retained
        // proposals, so the newest one is authoritative and a retraction can
        // walk it back. A LEGACY row is a fact this projection does not own and
        // cannot rebuild, so it takes the no-regress merge, and a delivered
        // package is never walked back by a mail the model misread as "shipped"
        // (a re-ship notice, a survey). Either way the carrier's own word, once
        // it has one, outranks the mail.
        let status = if managed {
            carrier_status.unwrap_or_else(|| info.status.as_str().to_string())
        } else {
            let current: String = conn.query_row(
                "SELECT status FROM shipments WHERE account_id=?1 AND id=?2",
                params![account, id],
                |r| r.get(0),
            )?;
            let current = ShipmentStatus::parse(&current).unwrap_or(ShipmentStatus::Shipped);
            carrier_status.unwrap_or_else(|| {
                ShipmentStatus::merge(current, info.status)
                    .as_str()
                    .to_string()
            })
        };
        record_newer_mail(
            conn, account, id, managed, *message, info, &status, *received,
        )?;
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

    fn poll_failures(store: &SqliteStore, id: i64) -> u32 {
        store
            .lock()
            .unwrap()
            .query_row(
                "SELECT poll_failures FROM shipments WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn retire(store: &SqliteStore, id: i64) {
        store
            .lock()
            .unwrap()
            .execute("UPDATE shipments SET poll_failures=5 WHERE id=?1", [id])
            .unwrap();
    }

    fn pollable(store: &SqliteStore, id: i64) -> bool {
        store
            .list_pollable_shipments(1, Utc::now() - chrono::Duration::days(45), 5)
            .unwrap()
            .iter()
            .any(|s| s.id == id)
    }

    /// A retired number is polled again once newer mail says it is real. Only a
    /// successful poll used to reset the counter, and a retired row never gets
    /// one, so retirement had quietly become permanent.
    #[test]
    fn newer_mail_un_retires_a_row_and_older_mail_does_not() {
        let store = fixture();
        // Owned row, minted from 30-day-old mail, then retired.
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE messages SET received_at=?1 WHERE id=1",
                [(Utc::now() - chrono::Duration::days(30)).to_rfc3339()],
            )
            .unwrap();
        write(&store, 1, None, &delivery("1Z999AA10123456784", "shipped"));
        let owned: i64 = store
            .lock()
            .unwrap()
            .query_row("SELECT id FROM shipments", [], |r| r.get(0))
            .unwrap();
        retire(&store, owned);
        assert!(!pollable(&store, owned), "retired: out of the poll queue");

        // Re-deciding the SAME mail is not news: still retired.
        write(&store, 1, None, &delivery("1Z999AA10123456784", "shipped"));
        assert_eq!(poll_failures(&store, owned), 5);

        // Newer mail is.
        write(
            &store,
            2,
            None,
            &delivery("1Z999AA10123456784", "out_for_delivery"),
        );
        assert_eq!(poll_failures(&store, owned), 0);
        assert!(pollable(&store, owned), "back in the poll queue");

        // The same for a legacy row.
        let legacy = legacy_row(&store, "1Z999AA10123456785", 20);
        retire(&store, legacy);
        write(
            &store,
            2,
            None,
            &delivery("1Z999AA10123456785", "out_for_delivery"),
        );
        assert_eq!(poll_failures(&store, legacy), 0);
        assert!(pollable(&store, legacy));
    }

    fn row_facts(store: &SqliteStore, id: i64) -> (String, String, Option<String>, Option<String>) {
        store
            .lock()
            .unwrap()
            .query_row(
                "SELECT status,carrier,delivered_at,tracking_url FROM shipments WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap()
    }

    /// A DELIVERED LEGACY ROW IS NEVER WALKED BACK. The model misreads a re-ship
    /// notice or a survey as "shipped": the row keeps its terminal status and
    /// its delivery time, and does not re-enter the poll queue. It is still
    /// news, so it still returns to the list and still un-retires.
    #[test]
    fn newer_mail_cannot_walk_a_delivered_legacy_row_back() {
        let store = fixture();
        let id = legacy_row(&store, "1Z999AA10123456784", 30);
        let landed = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE shipments SET status='delivered',delivered_at=?1 WHERE id=?2",
                params![landed, id],
            )
            .unwrap();
        write(&store, 2, None, &delivery("1Z999AA10123456784", "shipped"));
        let (status, _, delivered_at, _) = row_facts(&store, id);
        assert_eq!(
            status, "delivered",
            "terminal status survives a misread mail"
        );
        assert_eq!(delivered_at.as_deref(), Some(landed.as_str()));
        assert!(!pollable(&store, id), "a delivered package is not polled");
        assert_eq!(
            listed(&store).len(),
            0,
            "and en-route listings still exclude it"
        );
    }

    /// Newer mail that names a different carrier for the number re-routes the
    /// un-retired poll: five USPS rejections were USPS being asked about a
    /// FedEx label, and asking USPS five more times helps nobody.
    #[test]
    fn newer_mail_re_routes_a_legacy_row_to_the_carrier_it_names() {
        let store = fixture();
        let id = legacy_row(&store, "123456789012", 20);
        store
            .lock()
            .unwrap()
            .execute("UPDATE shipments SET carrier='usps' WHERE id=?1", [id])
            .unwrap();
        retire(&store, id);
        let mut mail = delivery("123456789012", "out_for_delivery");
        if let RecordProposal::Delivery { carrier, .. } = &mut mail.records[0] {
            *carrier = Some("fedex".into());
        }
        write(&store, 2, None, &mail);
        let (status, carrier, _, url) = row_facts(&store, id);
        assert_eq!(
            (status.as_str(), carrier.as_str()),
            ("out_for_delivery", "fedex")
        );
        assert!(
            url.unwrap().contains("fedex"),
            "the tracking link follows the carrier"
        );
        assert_eq!(poll_failures(&store, id), 0);
        assert!(pollable(&store, id));
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
