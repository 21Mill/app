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

/// Reset `trade`'s node to unknown when it is one of the user's own trade
/// keys (`is_own_key`). Before #809 the range remainder, the late create
/// confirmation, the late maker bond and the restored maker placeholder
/// stored the trade key there; a node is never one. Returns whether it
/// changed the row.
pub(crate) fn forget_own_key_node(
    trade: &mut TradeInfo,
    is_own_key: impl Fn(&str) -> bool,
) -> bool {
    if trade_node(trade).is_some_and(is_own_key) {
        trade.order.node_pubkey.clear();
        return true;
    }
    false
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
    fn an_own_trade_key_is_forgotten_as_a_node() {
        let mut row = trade("o1", 1, OrderStatus::Active, "aa");
        assert!(forget_own_key_node(&mut row, |k| k == "aa"));
        assert_eq!(trade_node(&row), None);
    }

    #[test]
    fn a_real_node_or_an_unknown_one_is_left_alone() {
        let mut row = trade("o1", 1, OrderStatus::Active, "node");
        assert!(!forget_own_key_node(&mut row, |k| k == "aa"));
        assert_eq!(trade_node(&row), Some("node"));
        let mut row = trade("o1", 1, OrderStatus::Active, "");
        assert!(!forget_own_key_node(&mut row, |_| true));
    }
}
