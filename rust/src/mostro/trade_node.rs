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
}
