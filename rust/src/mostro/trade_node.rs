//! The Mostro node a trade belongs to.
//!
//! A trade row records its issuing node in `order.node_pubkey`: the Kind
//! 38383 author for a taken book order, the node that answered for a row
//! built from a daemon message. Rows written before the node was recorded
//! hold an empty string, which means unknown: this helper never substitutes
//! the active node — a caller that does (push registration) says so. Every
//! reader that resolves a row's node goes through [`trade_node`].
//!
//! A step on a trade goes to that node, and only while it is the active one:
//! the step depends on what the app keeps for the active node alone (PoW,
//! protocol version, escrow mode, fee, bond policy). [`trade_action_node`] is
//! where every trade action takes its destination from (#810).
//!
//! A daemon message is heard according to who its author is to the trade it
//! is addressed to ([`speaker`], [`heard`]): the active node, and the trade's
//! own node whichever is active (#811), so a trade on another node moves to
//! its real status. Its chat stays on that node: [`refuse_other_node`].

use crate::api::types::TradeInfo;
use nostr_sdk::prelude::PublicKey;

/// The node that issued `trade`, or `None` when the row does not know it.
pub(crate) fn trade_node(trade: &TradeInfo) -> Option<&str> {
    let node = trade.order.node_pubkey.as_str();
    (!node.is_empty()).then_some(node)
}

/// The node a step on the trade `order_id` is sent to: the trade's own,
/// which must be the active node. A row on another node fails with
/// `TradeOnOtherNode` and nothing is sent. A row that does not know its node
/// goes to the active one, and no row at all is refused.
pub(crate) async fn trade_action_node(order_id: &str) -> anyhow::Result<PublicKey> {
    use crate::db::Storage as _;
    let db = crate::db::app_db::db().ok_or_else(|| anyhow::anyhow!("StorageUnavailable"))?;
    let trade = db.get_trade_by_order_id(order_id).await?;
    let active = crate::config::active_mostro_pubkey();
    // A row written before #817 may not know its node. Refusing it would
    // strand the trade (no release, no dispute), so it goes where it went
    // before #810: the active node, read the same way as push
    // (`mostro/push.rs`).
    let node = trade
        .as_ref()
        .map(|t| trade_node(t).unwrap_or(active.as_str()));
    action_node(node, &active).inspect_err(|_| {
        crate::api::logging::blog_warn(
            "orders",
            format!(
                "trade action refused for order={}: trade node={} active node={}",
                crate::api::logging::short_id(order_id),
                node.map_or("no row", crate::api::logging::short_id),
                crate::api::logging::short_id(&active),
            ),
        );
    })
}

/// Refuse a step on the trade `order_id` that belongs on its node without
/// being a daemon action — its chat, peer and dispute (#811): everything
/// about a trade happens on its own node. `TradeOnOtherNode` when the row
/// names a node other than the active one. A row that does not know its
/// node counts as the active node's, as for a trade action (#810), and no
/// row is let through, leaving the caller's own error. A row that cannot be
/// read is an error: its node is not known to be the active one.
pub(crate) async fn refuse_other_node(order_id: &str) -> anyhow::Result<()> {
    use crate::db::Storage as _;
    let Some(db) = crate::db::app_db::db() else {
        return Ok(());
    };
    let Some(trade) = db.get_trade_by_order_id(order_id).await? else {
        return Ok(());
    };
    let active = crate::config::active_mostro_pubkey();
    match trade_node(&trade) {
        Some(node) if !node.eq_ignore_ascii_case(&active) => {
            crate::api::logging::blog_warn(
                "orders",
                format!(
                    "trade step refused for order={}: trade node={} active node={}",
                    crate::api::logging::short_id(order_id),
                    crate::api::logging::short_id(node),
                    crate::api::logging::short_id(&active),
                ),
            );
            Err(anyhow::anyhow!("TradeOnOtherNode"))
        }
        _ => Ok(()),
    }
}

/// Who the author of a daemon message is to the trade it is addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Speaker {
    /// The active node.
    ActiveNode,
    /// The node that issued the trade, while another one is active.
    TradesNode,
    /// A node still owed a payout claim's traffic (docs/ANTI_ABUSE_BOND.md
    /// §6.4): heard about claims only.
    ClaimNode,
    /// Anyone else.
    Stranger,
}

/// Who `sender_hex`, the verified author of a daemon message addressed to
/// the trade key at `trade_index`, is to that trade.
///
/// The trade is a row that uses that key. A row that does not know its
/// node, or a key no row uses, has no node to hear: only the active one is,
/// as before #811. Anyone may p-tag a trade key, which is public, so the
/// author must be the node such a row names.
pub(crate) async fn speaker(sender_hex: &str, trade_index: u32) -> Speaker {
    if sender_hex.eq_ignore_ascii_case(&crate::config::active_mostro_pubkey()) {
        Speaker::ActiveNode
    } else if nodes_of_trade_key(trade_index)
        .await
        .iter()
        .any(|node| node.eq_ignore_ascii_case(sender_hex))
    {
        Speaker::TradesNode
    } else if crate::mostro::bond_claims::is_claim_node(sender_hex) {
        Speaker::ClaimNode
    } else {
        Speaker::Stranger
    }
}

/// The recorded nodes of the trade rows that use the trade key at
/// `trade_index`. Read off the rows, not the key bindings: a maker's key
/// stays bound to the provisional id its create was sent with, next to the
/// daemon's id, and that leftover has no row.
async fn nodes_of_trade_key(trade_index: u32) -> Vec<String> {
    use crate::db::Storage as _;
    let Some(db) = crate::db::app_db::db() else {
        return Vec::new();
    };
    db.list_trades()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|t| t.trade_key_index == trade_index)
        .filter_map(|t| trade_node(t).map(str::to_owned))
        .collect()
}

/// Whether a message from `speaker` is heard: the active node's and the
/// trade's own always, a claim node's about claims only, anyone else's never.
pub(crate) fn heard(speaker: Speaker, claim_action: bool) -> bool {
    match speaker {
        Speaker::ActiveNode | Speaker::TradesNode => true,
        Speaker::ClaimNode => claim_action,
        Speaker::Stranger => false,
    }
}

/// Whether a message from `speaker`, `sender_hex`, addressed to the trade
/// key at `trade_index` and naming the order `order_id`, is heard.
///
/// The trade's own node speaks only about the trade it issued under that
/// key: the order must have a row that uses the key and names the node.
/// Otherwise its key would open any order it names, another node's included,
/// since the generation gate refuses only an older key. Its claim traffic
/// keeps the claim node's terms (§6.4): a claim outlives its row. The active
/// node and a claim node are heard as [`heard`] says.
pub(crate) async fn heard_about(
    speaker: Speaker,
    claim_action: bool,
    sender_hex: &str,
    trade_index: u32,
    order_id: Option<&str>,
) -> bool {
    if speaker != Speaker::TradesNode {
        return heard(speaker, claim_action);
    }
    issued_under(order_id, sender_hex, trade_index).await
        || (claim_action && crate::mostro::bond_claims::is_claim_node(sender_hex))
}

/// Whether the order `order_id` has a row that uses the trade key at
/// `trade_index` and names `node` as its node.
async fn issued_under(order_id: Option<&str>, node: &str, trade_index: u32) -> bool {
    use crate::db::Storage as _;
    let (Some(order_id), Some(db)) = (order_id, crate::db::app_db::db()) else {
        return false;
    };
    let Ok(Some(trade)) = db.get_trade_by_order_id(order_id).await else {
        return false;
    };
    trade.trade_key_index == trade_index
        && trade_node(&trade).is_some_and(|n| n.eq_ignore_ascii_case(node))
}

/// `node` when it is the `active` one, else `TradeOnOtherNode`.
fn action_node(node: Option<&str>, active: &str) -> anyhow::Result<PublicKey> {
    match node {
        Some(node) if node.eq_ignore_ascii_case(active) => Ok(PublicKey::from_hex(node)?),
        _ => Err(anyhow::anyhow!("TradeOnOtherNode")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::OrderStatus;
    use crate::mostro::test_fixtures::trade;

    #[test]
    fn a_recorded_node_is_the_trades_node() {
        let row = trade("o1", 1, OrderStatus::Active, "aa");
        assert_eq!(trade_node(&row), Some("aa"));
    }

    #[test]
    fn an_empty_node_is_unknown() {
        let row = trade("o1", 1, OrderStatus::Active, "");
        assert_eq!(trade_node(&row), None);
    }

    #[test]
    fn a_step_goes_to_the_trades_node_when_it_is_the_active_one() {
        let node = nostr_sdk::prelude::Keys::generate().public_key();
        let hex = node.to_hex();

        assert_eq!(action_node(Some(&hex), &hex).unwrap(), node);
        assert_eq!(
            action_node(Some(&hex.to_uppercase()), &hex).unwrap(),
            node,
            "a node is the same node in either case"
        );
    }

    #[test]
    fn a_step_on_another_node_or_without_a_row_is_refused() {
        let active = nostr_sdk::prelude::Keys::generate().public_key().to_hex();
        let other = nostr_sdk::prelude::Keys::generate().public_key().to_hex();

        for node in [Some(other.as_str()), None] {
            let err = action_node(node, &active).unwrap_err();
            assert_eq!(err.to_string(), "TradeOnOtherNode", "node {node:?}");
        }
    }

    /// A row written before #817 may not know its node: a step on it goes to
    /// the active node, as it did before #810, rather than stranding the
    /// trade.
    #[tokio::test]
    async fn a_step_on_a_row_without_a_node_goes_to_the_active_one() {
        use crate::mostro::test_fixtures::{forget_trade_row, store_trade_row};
        // Arrange
        let order_id = uuid::Uuid::new_v4().to_string();
        store_trade_row(&trade(&order_id, 7, OrderStatus::FiatSent, "")).await;

        // Act
        let node = trade_action_node(&order_id).await;
        forget_trade_row(&order_id).await;

        // Assert
        assert_eq!(
            node.unwrap().to_hex(),
            crate::config::active_mostro_pubkey().to_lowercase(),
        );
    }

    /// The message builders of a step on an existing trade (#810).
    const TRADE_ACTIONS: [&str; 9] = [
        "add_invoice",
        "fiat_sent",
        "release",
        "cancel",
        "dispute",
        "rate_user",
        "add_cashu_escrow",
        "take_buy",
        "take_sell",
    ];

    /// The functions that build one, and how each learns where to send it.
    /// `take_order_once` takes a book order, which is on the active node by
    /// construction; the bond payout claim (`add_bond_invoice`) is not here,
    /// it goes to the node that issued the claim.
    const CHECKS_ITS_NODE: [&str; 7] = [
        "send_invoice",
        "send_fiat_sent",
        "release_order",
        "cancel_order",
        "request_bond_invoice_again",
        "open_dispute",
        "submit_rating",
    ];
    const IS_HANDED_ITS_NODE: [&str; 2] = ["cancel_maker_bond", "submit_escrow"];
    /// What hands `submit_escrow` its node (`cancel_order` hands
    /// `cancel_maker_bond` its own, and is listed above already).
    const CHECKS_THE_NODE_IT_HANDS: [&str; 1] = ["lock_escrow"];
    const TAKES_A_BOOK_ORDER: &str = "take_order_once";

    /// The production half of every Rust source but the builders themselves
    /// and the generated bridge, by path.
    fn production_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let source = std::fs::read_to_string(&path).unwrap();
                    let production = source
                        .split("\n#[cfg(test)]\nmod tests {")
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    out.push((path.display().to_string(), production));
                }
            }
        }
        let mut sources = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut sources,
        );
        sources.retain(|(path, _)| {
            !path.ends_with("mostro/actions.rs") && !path.ends_with("frb_generated.rs")
        });
        sources
    }

    /// The name of the function `source[..at]` ends inside.
    fn enclosing_fn(source: &str, at: usize) -> String {
        let line = source[..at]
            .lines()
            .rev()
            .map(str::trim_start)
            .find(|line| {
                let line = line
                    .trim_start_matches("pub(crate) ")
                    .trim_start_matches("pub ");
                line.starts_with("fn ") || line.starts_with("async fn ")
            })
            .expect("a call sits inside a function");
        let name = &line[line.find("fn ").unwrap() + 3..];
        name[..name.find(['(', '<']).unwrap()].to_string()
    }

    /// The body of top-level function `name` in `source`, if it is there.
    fn fn_body<'a>(source: &'a str, name: &str) -> Option<&'a str> {
        let start = source.find(&format!("fn {name}("))?;
        let len = source[start..].find("\n}\n")?;
        Some(&source[start..start + len])
    }

    /// #810: a trade action is only built where its destination is the
    /// trade's own node, checked against the active one — never read from
    /// the active node alone. A new caller has to say where it sends.
    #[test]
    fn every_trade_action_is_sent_to_the_trades_own_node() {
        let sources = production_sources();
        let mut callers = std::collections::BTreeSet::new();
        for (path, source) in &sources {
            for action in TRADE_ACTIONS {
                let call = format!("actions::{action}(");
                for (at, _) in source.match_indices(&call) {
                    let caller = enclosing_fn(source, at);
                    assert!(
                        CHECKS_ITS_NODE.contains(&caller.as_str())
                            || IS_HANDED_ITS_NODE.contains(&caller.as_str())
                            || caller == TAKES_A_BOOK_ORDER,
                        "{path}: `{caller}` builds `{action}` without checking the \
                         trade's node (#810): resolve it with `trade_action_node`",
                    );
                    callers.insert(caller);
                }
            }
        }
        // Every listed caller is still one, so the lists cannot go stale.
        for name in CHECKS_ITS_NODE.iter().chain(&IS_HANDED_ITS_NODE) {
            assert!(
                callers.contains(*name),
                "`{name}` builds no trade action any more"
            );
        }

        for (name, checks) in CHECKS_ITS_NODE
            .iter()
            .chain(&CHECKS_THE_NODE_IT_HANDS)
            .map(|name| (name, true))
            .chain(IS_HANDED_ITS_NODE.iter().map(|name| (name, false)))
        {
            let body = sources
                .iter()
                .find_map(|(_, source)| fn_body(source, name))
                .unwrap_or_else(|| panic!("`{name}` exists"));
            assert!(
                !body.contains("active_mostro_pubkey"),
                "`{name}` sends to the active node instead of the trade's",
            );
            if checks {
                assert!(
                    body.contains("trade_action_node("),
                    "`{name}` does not check the trade's node",
                );
            }
        }
    }

    /// A trade key index no other test binds: a shared one would resolve to
    /// another test's row.
    fn unbound_trade_index() -> u32 {
        (uuid::Uuid::new_v4().as_u128() as u32) | 1 << 31
    }

    /// #811: the node that issued a trade is heard about it, whichever node
    /// is active, so a trade on another node moves to its real status.
    #[tokio::test]
    async fn a_message_from_the_trades_own_node_is_heard() {
        use crate::mostro::test_fixtures::{forget_trade_row, store_trade_row};
        // Arrange
        let node = nostr_sdk::prelude::Keys::generate().public_key().to_hex();
        let order_id = uuid::Uuid::new_v4().to_string();
        let index = unbound_trade_index();
        store_trade_row(&trade(&order_id, index, OrderStatus::Active, &node)).await;

        // Act
        let speaker = speaker(&node, index).await;
        forget_trade_row(&order_id).await;

        // Assert
        assert!(heard(speaker, false), "the trade's node: got {speaker:?}");
    }

    /// #811: a maker's trade key stays bound to the provisional id its
    /// create was sent with, next to the id the daemon gave the order. That
    /// leftover has no row, and must not hide the trade's node.
    #[tokio::test]
    async fn a_leftover_binding_of_the_trade_key_does_not_hide_its_node() {
        use crate::db::Storage as _;
        use crate::mostro::test_fixtures::{forget_trade_row, store_trade_row};
        // Arrange — the provisional binding first, as a create writes it.
        let node = nostr_sdk::prelude::Keys::generate().public_key().to_hex();
        let index = unbound_trade_index();
        let provisional = uuid::Uuid::new_v4().to_string();
        let order_id = uuid::Uuid::new_v4().to_string();
        let row = trade(&order_id, index, OrderStatus::Active, &node);
        store_trade_row(&row).await;
        let db = crate::db::app_db::db().unwrap();
        db.delete_trade_key(&order_id).await.unwrap();
        db.save_trade_key(&provisional, index).await.unwrap();
        db.save_trade_key(&order_id, index).await.unwrap();

        // Act
        let speaker = speaker(&node, index).await;
        db.delete_trade_key(&provisional).await.unwrap();
        forget_trade_row(&order_id).await;

        // Assert
        assert!(heard(speaker, false), "the trade's node: got {speaker:?}");
    }

    /// #811: no other node is heard about a trade: not one that issued none
    /// of it, and none at all for a row that does not know its node or a
    /// key bound to no row. The active node still is.
    #[tokio::test]
    async fn only_the_active_node_and_the_trades_own_are_heard() {
        use crate::mostro::test_fixtures::{forget_trade_row, store_trade_row};
        let node = nostr_sdk::prelude::Keys::generate().public_key().to_hex();
        let other = nostr_sdk::prelude::Keys::generate().public_key().to_hex();
        let active = crate::config::active_mostro_pubkey();
        let on_node = (uuid::Uuid::new_v4().to_string(), unbound_trade_index());
        let unknown = (uuid::Uuid::new_v4().to_string(), unbound_trade_index());
        store_trade_row(&trade(&on_node.0, on_node.1, OrderStatus::Active, &node)).await;
        store_trade_row(&trade(&unknown.0, unknown.1, OrderStatus::Active, "")).await;

        let cases = [
            ("another node", other.as_str(), on_node.1, false),
            ("a row without a node", node.as_str(), unknown.1, false),
            (
                "a key bound to no row",
                node.as_str(),
                unbound_trade_index(),
                false,
            ),
            ("the active node", active.as_str(), on_node.1, true),
        ];
        let mut outcomes = Vec::new();
        for (case, sender, index, expected) in cases {
            let speaker = speaker(sender, index).await;
            outcomes.push((case, speaker, expected));
        }
        forget_trade_row(&on_node.0).await;
        forget_trade_row(&unknown.0).await;

        for (case, speaker, expected) in outcomes {
            assert_eq!(heard(speaker, false), expected, "{case}: got {speaker:?}");
        }
    }

    /// #811: the trade's own node is heard only about the trade it issued
    /// under that key. Its key opens nothing else: a message naming another
    /// trade's order (another node's, under a lower key), no order, or an
    /// order with no row is refused. The active node is unchanged.
    #[tokio::test]
    async fn a_trades_node_is_heard_only_about_its_own_order() {
        use crate::mostro::test_fixtures::{forget_trade_row, store_trade_row};
        // Arrange — node A's trade under a newer key, node B's under an
        // older one: the generation gate alone lets A's key through to B's.
        let node_a = nostr_sdk::prelude::Keys::generate().public_key().to_hex();
        let node_b = nostr_sdk::prelude::Keys::generate().public_key().to_hex();
        let index_b = unbound_trade_index() & !1;
        let index_a = index_b + 1;
        let order_a = uuid::Uuid::new_v4().to_string();
        let order_b = uuid::Uuid::new_v4().to_string();
        store_trade_row(&trade(&order_a, index_a, OrderStatus::Active, &node_a)).await;
        store_trade_row(&trade(&order_b, index_b, OrderStatus::Active, &node_b)).await;
        let unknown = uuid::Uuid::new_v4().to_string();
        let active = crate::config::active_mostro_pubkey();

        let cases = [
            (
                "its own order",
                node_a.as_str(),
                Some(order_a.as_str()),
                true,
            ),
            (
                "another node's order",
                node_a.as_str(),
                Some(order_b.as_str()),
                false,
            ),
            ("no order", node_a.as_str(), None, false),
            (
                "an order with no row",
                node_a.as_str(),
                Some(unknown.as_str()),
                false,
            ),
            (
                "the active node",
                active.as_str(),
                Some(order_b.as_str()),
                true,
            ),
        ];

        // Act
        let mut outcomes = Vec::new();
        for (case, sender, order, expected) in cases {
            let speaker = speaker(sender, index_a).await;
            let heard = heard_about(speaker, false, sender, index_a, order).await;
            outcomes.push((case, heard, expected));
        }
        forget_trade_row(&order_a).await;
        forget_trade_row(&order_b).await;

        // Assert
        for (case, heard, expected) in outcomes {
            assert_eq!(heard, expected, "{case}");
        }
    }

    /// The envelope builders of a trade's chat, peer and dispute alike
    /// (#811).
    const CHAT_ENVELOPES: [&str; 2] = ["mostro_wrap", "mostro_wrap_reaction"];
    /// The functions that only pass a chat send through, to their callers.
    const CHAT_RELAYS: [&str; 4] = [
        "publish_chat_payload",
        "publish_chat_payload_for",
        "publish_delivered_chat_payload_for",
        "send_attachment",
    ];
    /// Where a trade's chat is sent from: each refuses a trade of another
    /// node.
    const CHAT_SENDERS: [&str; 6] = [
        "send_message",
        "send_reaction",
        "send_file",
        "submit_evidence",
        "share_chat_key_with_solver",
        "send_dispute_file",
    ];

    /// #811: everything about a trade happens on its own node, its chat
    /// included. Every chat send goes through a sender that refuses a trade
    /// of another node; a new path has to be one of them.
    #[test]
    fn every_chat_send_refuses_a_trade_of_another_node() {
        let sources = production_sources();
        let mut pending: Vec<String> = CHAT_ENVELOPES
            .iter()
            .map(|e| format!("transport::{e}("))
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        let mut senders = std::collections::BTreeSet::new();
        while let Some(call) = pending.pop() {
            for (path, source) in &sources {
                for (at, _) in source.match_indices(&call) {
                    if source[..at].ends_with("fn ") {
                        continue;
                    }
                    let caller = enclosing_fn(source, at);
                    if CHAT_RELAYS.contains(&caller.as_str()) {
                        if seen.insert(caller.clone()) {
                            pending.push(format!("{caller}("));
                        }
                    } else {
                        assert!(
                            CHAT_SENDERS.contains(&caller.as_str()),
                            "{path}: `{caller}` sends a trade's chat without refusing \
                             another node's trade: add the check and list it"
                        );
                        senders.insert((path.clone(), caller));
                    }
                }
            }
        }
        for name in CHAT_SENDERS {
            let (path, _) = senders
                .iter()
                .find(|(_, caller)| caller == name)
                .unwrap_or_else(|| panic!("`{name}` sends no chat any more: unlist it"));
            let source = &sources.iter().find(|(p, _)| p == path).unwrap().1;
            let body = fn_body(source, name).unwrap();
            assert!(
                body.contains("trade_node::refuse_other_node("),
                "{path}: `{name}` must refuse a trade of another node"
            );
        }
    }
}
