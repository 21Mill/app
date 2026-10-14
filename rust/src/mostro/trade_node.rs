//! The Mostro node a trade belongs to.
//!
//! A trade row records its issuing node in `order.node_pubkey`: the Kind
//! 38383 author for a taken book order, the node that answered for a row
//! built from a daemon message. Rows written before the node was recorded
//! hold an empty string, which means unknown: this helper never substitutes
//! the active node — a caller that does (push registration) says so. Every
//! reader that resolves a row's node goes through [`trade_node`].

use crate::api::types::TradeInfo;

/// The node that issued `trade`, or `None` when the row does not know it.
pub(crate) fn trade_node(trade: &TradeInfo) -> Option<&str> {
    let node = trade.order.node_pubkey.as_str();
    (!node.is_empty()).then_some(node)
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
}
