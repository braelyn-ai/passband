//! Names, merchants and orders on agent delivery records, and the one rule for
//! which packages are the same purchase.
//!
//! The model's words are UNTRUSTED text lifted from email, so every field is
//! laundered here before it reaches a row: the item name through the same
//! sanitizer the old shipments extractor used, the merchant through a shape and
//! identity check, and each order reference through `sanitize_order_ref`.
//!
//! GROUPING. Two packages are one card when they share an order in a KNOWN
//! merchant's namespace: the same `(merchant_key, order_key)` with a non-empty
//! merchant key. Transitive, so a box carrying orders 1 and 2 and a box carrying
//! 2 and 3 are one card. An order with no merchant never groups: "#1001" is a
//! different purchase at every shop that numbers its orders from 1000.
use crate::triage::extract::shipments::{sanitize_item_name, sanitize_order_ref};
use crate::types::{Shipment, ShipmentLeg, ShipmentOrder};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// The merchant half of an order's identity: lowercase alphanumerics, so
/// "Bill's Exhausts" and "BILLS EXHAUSTS" are one namespace.
pub fn merchant_key(merchant: &str) -> String {
    merchant
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The order half: uppercase alphanumerics of the SANITIZED reference, so
/// "#21470", "Order 21470" and "21470" are one order.
pub fn order_key(order_ref: &str) -> String {
    order_ref
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

/// Carrier names, as merchant keys. A carrier delivers the box; it did not sell
/// it. Amazon is deliberately absent: it is both, and "Amazon" is a real store.
const CARRIER_KEYS: &[&str] = &[
    "ups",
    "unitedparcelservice",
    "usps",
    "uspostalservice",
    "unitedstatespostalservice",
    "fedex",
    "federalexpress",
    "dhl",
    "dhlexpress",
    "ontrac",
    "lasership",
];

/// Platforms that send a store's mail under their own name. The customer knows
/// the store, and every store on the platform would otherwise share one
/// namespace, which is exactly the "#1001" collision grouping must not make.
const RELAY_KEYS: &[&str] = &[
    "shopify",
    "shopifyemail",
    "shop",
    "shopapp",
    "bigcommerce",
    "squarespace",
    "wix",
    "klaviyo",
    "mailchimp",
];

/// Strip control and bidi characters and collapse whitespace.
fn plain(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control() || c.is_whitespace())
        .filter(|c| {
            !matches!(
                c,
                '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A model-emitted merchant reduced to a storable store name, or `None`. Never
/// a carrier, never a relay platform, never something with no letters or
/// digits in it, and never longer than 60 characters.
pub fn sanitize_merchant(raw: Option<&str>) -> Option<String> {
    let cleaned: String = plain(raw?).chars().take(60).collect();
    let cleaned = cleaned.trim().to_string();
    let key = merchant_key(&cleaned);
    if key.is_empty() || CARRIER_KEYS.contains(&key.as_str()) || RELAY_KEYS.contains(&key.as_str())
    {
        return None;
    }
    Some(cleaned)
}

/// Model-emitted order references, sanitized and deduped by [`order_key`].
pub fn sanitize_order_refs(raw: &[String]) -> Vec<String> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for candidate in raw {
        if let Some(clean) = sanitize_order_ref(Some(candidate)) {
            let key = order_key(&clean);
            if !key.is_empty() && !seen.contains(&key) {
                seen.push(key);
                out.push(clean);
            }
        }
    }
    out
}

/// A model-emitted item name reduced to a display name, or `None`.
/// [`sanitize_item_name`] does the laundering (controls, subject echoes, URLs,
/// carrier names, status prose, the length cap). On top of that, a name that is
/// only the store, the carrier or one of the order numbers says nothing about
/// what is in the box, and the card already shows each of those on its own.
pub fn sanitize_agent_item_name(
    raw: Option<&str>,
    subject: &str,
    merchant: Option<&str>,
    carrier: &str,
    order_refs: &[String],
) -> Option<String> {
    let name = sanitize_item_name(raw, subject)?;
    let key = merchant_key(&name);
    if merchant.is_some_and(|m| merchant_key(m) == key)
        || merchant_key(carrier) == key
        || CARRIER_KEYS.contains(&key.as_str())
    {
        return None;
    }
    let as_order = sanitize_order_ref(Some(&name)).map(|r| order_key(&r));
    if as_order.is_some_and(|k| order_refs.iter().any(|r| order_key(r) == k)) {
        return None;
    }
    Some(name)
}

/// The grouping keys of a package's orders: merchant-less orders are dropped
/// here, which is the whole of "a bare ref never groups".
fn grouping_keys(orders: &[ShipmentOrder]) -> Vec<(String, String)> {
    orders
        .iter()
        .filter_map(|o| {
            let merchant = merchant_key(o.merchant.as_deref().unwrap_or(""));
            let order = order_key(&o.order_ref);
            (!merchant.is_empty() && !order.is_empty()).then_some((merchant, order))
        })
        .collect()
}

/// Partition packages into purchase groups (union-find over shared keys).
/// Groups come back in order of their first member, members in input order, so
/// a caller's sort survives.
pub fn order_groups(members: &[&[ShipmentOrder]]) -> Vec<Vec<usize>> {
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut parent: Vec<usize> = (0..members.len()).collect();
    let mut owner: HashMap<(String, String), usize> = HashMap::new();
    for (i, orders) in members.iter().enumerate() {
        for key in grouping_keys(orders) {
            match owner.get(&key) {
                Some(&j) => {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        // Root at the smaller index so first-appearance order holds.
                        parent[a.max(b)] = a.min(b);
                    }
                }
                None => {
                    owner.insert(key, i);
                }
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut slot: HashMap<usize, usize> = HashMap::new();
    for i in 0..members.len() {
        let root = find(&mut parent, i);
        let at = *slot.entry(root).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[at].push(i);
    }
    groups
}

/// The union of several packages' orders, deduped by key. A merchant-less
/// order dedupes only against other merchant-less orders with the same number.
pub fn union_orders<'a>(
    lists: impl IntoIterator<Item = &'a [ShipmentOrder]>,
) -> Vec<ShipmentOrder> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for list in lists {
        for order in list {
            let key = (
                merchant_key(order.merchant.as_deref().unwrap_or("")),
                order_key(&order.order_ref),
            );
            if !seen.contains(&key) {
                seen.push(key);
                out.push(order.clone());
            }
        }
    }
    out
}

/// Who stands for a group on its card: THE NEWEST PACKAGE STILL ON ITS WAY,
/// and the newest overall only when every package has landed. "Newest" is
/// `last_update`, then the higher row id.
///
/// Newest-overall alone is wrong: an order whose first box was delivered
/// yesterday while its second is still in transit would put the delivered box
/// on the card, and every surface that drops a delivered card (the client's
/// "delivered today" rail, `include_delivered = false`) would hide a package
/// that is still coming. Both doors pick with this one function.
///
/// Returns `members` reordered: the representative first, then the legs,
/// newest first.
pub fn representative_first<T>(
    mut members: Vec<T>,
    key: impl Fn(&T) -> (bool, DateTime<Utc>, i64),
) -> Vec<T> {
    // Legs newest first...
    members.sort_by(|a, b| {
        let (_, at_a, id_a) = key(a);
        let (_, at_b, id_b) = key(b);
        at_b.cmp(&at_a).then_with(|| id_b.cmp(&id_a))
    });
    // ...and the representative is the first one not delivered, else the first.
    if let Some(at) = members.iter().position(|m| !key(m).0) {
        let rep = members.remove(at);
        members.insert(0, rep);
    }
    members
}

/// Fold items into purchase groups, representative first in each. The one
/// grouping both doors run: `orders` reads an item's order links, `key` its
/// `(delivered, last_update, row id)` for [`representative_first`].
pub fn fold_groups<T>(
    items: Vec<T>,
    orders: impl Fn(&T) -> &[ShipmentOrder],
    key: impl Fn(&T) -> (bool, DateTime<Utc>, i64) + Copy,
) -> Vec<Vec<T>> {
    let groups = {
        let lists: Vec<&[ShipmentOrder]> = items.iter().map(&orders).collect();
        order_groups(&lists)
    };
    let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
    groups
        .into_iter()
        .map(|group| {
            let members: Vec<T> = group.into_iter().filter_map(|i| slots[i].take()).collect();
            representative_first(members, key)
        })
        .filter(|members| !members.is_empty())
        .collect()
}

/// The name and merchant a card shows: the representative's when it has
/// them, else the newest non-empty one among the legs. `members` is in
/// [`representative_first`] order.
pub fn card_name_and_merchant<'a>(
    members: impl Iterator<Item = (&'a str, Option<&'a str>)> + Clone,
) -> (String, Option<String>) {
    let name = members
        .clone()
        .map(|(n, _)| n.trim())
        .find(|n| !n.is_empty())
        .unwrap_or("")
        .to_string();
    let merchant = members
        .filter_map(|(_, m)| m.map(str::trim))
        .find(|m| !m.is_empty())
        .map(str::to_string);
    (name, merchant)
}

/// Collapse listing rows that share an order into one card each (see
/// [`fold_groups`] and [`representative_first`]). The representative's status,
/// carrier, tracking, eta and thread are the card's; the other members become
/// `legs`; the orders are the union.
pub fn group_shipments(rows: Vec<Shipment>) -> Vec<Shipment> {
    let groups = fold_groups(
        rows,
        |r| r.orders.as_slice(),
        |r| (r.status == "delivered", r.last_update, r.id),
    );
    let mut out = Vec::with_capacity(groups.len());
    for members in groups {
        let orders = union_orders(members.iter().map(|m| m.orders.as_slice()));
        let (item_name, merchant) = card_name_and_merchant(
            members
                .iter()
                .map(|m| (m.item_name.as_str(), m.merchant.as_deref())),
        );
        let mut members = members.into_iter();
        let Some(mut card) = members.next() else {
            continue;
        };
        card.legs = members
            .map(|m| ShipmentLeg {
                id: m.id,
                carrier: m.carrier,
                tracking_number: m.tracking_number,
                status: m.status,
                tracking_url: m.tracking_url,
                last_update: m.last_update,
                delivered_at: m.delivered_at,
            })
            .collect();
        card.item_name = item_name;
        card.merchant = merchant;
        card.orders = orders;
        out.push(card);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn order(merchant: Option<&str>, r: &str) -> ShipmentOrder {
        ShipmentOrder {
            merchant: merchant.map(str::to_string),
            order_ref: r.into(),
        }
    }

    fn row(id: i64, age_hours: i64, name: &str, orders: Vec<ShipmentOrder>) -> Shipment {
        let at = Utc::now() - Duration::hours(age_hours);
        Shipment {
            id,
            account_id: 1,
            tracking_number: format!("1Z{id:016}"),
            carrier: "ups".into(),
            item_name: name.into(),
            status: "shipped".into(),
            tracking_url: None,
            thread_id: Some(format!("t{id}")),
            first_seen: at,
            last_update: at,
            carrier_status_raw: None,
            eta: None,
            delivered_at: None,
            last_polled_at: None,
            poll_failures: 0,
            last_answered_at: None,
            merchant: orders.first().and_then(|o| o.merchant.clone()),
            orders,
            legs: Vec::new(),
        }
    }

    #[test]
    fn keys_fold_spelling() {
        assert_eq!(merchant_key("Bill's Exhausts"), "billsexhausts");
        assert_eq!(merchant_key("BILLS EXHAUSTS"), "billsexhausts");
        assert_eq!(order_key("#21470"), "21470");
        assert_eq!(order_key("a1-b2"), "A1B2");
    }

    #[test]
    fn grouping_is_transitive() {
        let a = row(
            1,
            3,
            "",
            vec![order(Some("Shop"), "1"), order(Some("Shop"), "2")],
        );
        let b = row(
            2,
            2,
            "",
            vec![order(Some("SHOP"), "#2"), order(Some("Shop"), "3")],
        );
        let c = row(3, 1, "", vec![order(Some("Shop"), "3")]);
        let d = row(4, 0, "", vec![order(Some("Other"), "1")]);
        let cards = group_shipments(vec![a, b, c, d]);
        assert_eq!(
            cards.len(),
            2,
            "A-B-C chain through 2 and 3; D is another store"
        );
        let chain = cards.iter().find(|c| c.id == 3).unwrap();
        assert_eq!(chain.legs.len(), 2);
        assert_eq!(chain.orders.len(), 3, "union deduped by key");
    }

    #[test]
    fn a_merchantless_ref_never_groups() {
        let a = row(1, 2, "", vec![order(None, "1001")]);
        let b = row(2, 1, "", vec![order(None, "1001")]);
        assert_eq!(group_shipments(vec![a, b]).len(), 2);
    }

    #[test]
    fn the_newest_member_represents_and_names_fall_back() {
        let old = row(
            1,
            5,
            "Austin Racing DB Killer AUR10",
            vec![order(Some("Bill's Exhausts"), "21470")],
        );
        let mut new = row(2, 1, "", vec![order(Some("BILLS EXHAUSTS"), "#21470")]);
        new.merchant = None;
        let cards = group_shipments(vec![old, new]);
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        assert_eq!(card.id, 2, "newest last_update represents");
        assert_eq!(
            card.item_name, "Austin Racing DB Killer AUR10",
            "name from the group"
        );
        assert_eq!(card.merchant.as_deref(), Some("Bill's Exhausts"));
        assert_eq!(card.legs.len(), 1);
        assert_eq!(card.legs[0].id, 1);
        assert_eq!(card.orders.len(), 1);
    }

    /// A DELIVERED BOX NEVER STANDS FOR ONE STILL COMING. The newer box landed
    /// two days ago; the older one is still in transit. The card is the one in
    /// transit, or every surface that drops delivered cards hides it.
    #[test]
    fn an_undelivered_member_represents_over_a_newer_delivered_one() {
        let mut landed = row(1, 48, "", vec![order(Some("Shop"), "5")]);
        landed.status = "delivered".into();
        landed.delivered_at = Some(landed.last_update);
        let coming = row(2, 72, "", vec![order(Some("Shop"), "5")]);
        let cards = group_shipments(vec![landed, coming]);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, 2, "the box still on its way represents");
        assert_eq!(cards[0].status, "shipped");
        assert_eq!(cards[0].legs[0].id, 1);

        // Every box landed: the newest represents.
        let mut a = row(1, 48, "", vec![order(Some("Shop"), "5")]);
        let mut b = row(2, 72, "", vec![order(Some("Shop"), "5")]);
        a.status = "delivered".into();
        b.status = "delivered".into();
        assert_eq!(group_shipments(vec![b, a])[0].id, 1);
    }

    #[test]
    fn a_tie_goes_to_the_higher_id_and_its_own_name_wins() {
        let mut a = row(1, 1, "Older name", vec![order(Some("S"), "9")]);
        let mut b = row(2, 1, "Own name", vec![order(Some("S"), "9")]);
        let at = Utc::now();
        a.last_update = at;
        b.last_update = at;
        let cards = group_shipments(vec![a, b]);
        assert_eq!(cards[0].id, 2);
        assert_eq!(cards[0].item_name, "Own name");
    }

    #[test]
    fn merchant_refuses_carriers_and_relays() {
        assert_eq!(sanitize_merchant(Some("UPS")), None);
        assert_eq!(sanitize_merchant(Some("Shopify")), None);
        assert_eq!(sanitize_merchant(Some("  ")), None);
        assert_eq!(
            sanitize_merchant(Some("Bill's\u{202E} Exhausts")).as_deref(),
            Some("Bill's Exhausts")
        );
        assert_eq!(sanitize_merchant(Some("Amazon")).as_deref(), Some("Amazon"));
    }

    #[test]
    fn a_name_that_is_the_store_carrier_or_order_is_dropped() {
        let refs = vec!["21470".to_string()];
        let name = |raw: &str| {
            sanitize_agent_item_name(
                Some(raw),
                "Your order shipped",
                Some("Bill's Exhausts"),
                "ups",
                &refs,
            )
        };
        assert_eq!(name("Bills Exhausts"), None);
        assert_eq!(name("#21470"), None);
        // A ref the item laundering alone would keep, as a product code.
        let lettered = vec!["BX-21470".to_string()];
        assert_eq!(
            sanitize_agent_item_name(Some("BX-21470"), "Shipped", None, "ups", &lettered),
            None
        );
        assert!(sanitize_agent_item_name(Some("BX-21470"), "Shipped", None, "ups", &[]).is_some());
        assert_eq!(name("Arriving soon"), None);
        assert_eq!(
            name("Austin Racing DB Killer AUR10").as_deref(),
            Some("Austin Racing DB Killer AUR10")
        );
    }

    #[test]
    fn refs_are_sanitized_and_deduped() {
        let refs = sanitize_order_refs(&[
            "Order #21470".into(),
            "21470".into(),
            "https://evil.example/x".into(),
        ]);
        assert_eq!(refs, vec!["21470".to_string()]);
    }
}
