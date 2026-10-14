//! When the escrow deadline guardian is estimated to act on a trade.
//!
//! The estimate is [`crate::mostro::escrow_deadline`]'s window, counted from
//! the moment the trade's escrow locked (`TradeInfo::escrow_locked_at`), with
//! the window of the node that sent that message, whichever node is active
//! now. Dart shows it as an approximate countdown; it decides nothing.

use crate::api::types::{OrderStatus, TradeInfo};
use crate::db::Storage;
use crate::mostro::escrow_deadline;

/// Unix seconds at which the node is estimated to act on `order_id`'s trade:
/// cancel it in `active`, open a dispute in `fiat-sent`. `None` in any other
/// status, when the lock time was never recorded (a trade from before it
/// was), or when the node has no window (Cashu, a node without the
/// guardian, values it would not run with).
pub async fn trade_escrow_deadline(order_id: String) -> Option<i64> {
    let db = crate::db::app_db::db()?;
    let trade = db.get_trade_by_order_id(&order_id).await.ok().flatten()?;
    let (locked_at, node) = escrow_lock_of(&trade)?;
    let window = escrow_deadline::for_node(node).await?;
    Some(locked_at.saturating_add(window.secs()))
}

/// When `trade`'s escrow locked and the node that said so, while the
/// guardian watches it (`active`, `fiat-sent`).
fn escrow_lock_of(trade: &TradeInfo) -> Option<(i64, &str)> {
    if !matches!(
        trade.order.status,
        OrderStatus::Active | OrderStatus::FiatSent
    ) {
        return None;
    }
    Some((trade.escrow_locked_at?, trade.escrow_node.as_deref()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{BuyerStep, OrderInfo, OrderKind, TradeRole, TradeStep};

    fn trade(order_id: &str, status: OrderStatus) -> TradeInfo {
        TradeInfo {
            id: order_id.into(),
            order: OrderInfo {
                id: order_id.into(),
                kind: OrderKind::Sell,
                status,
                amount_sats: Some(100_000),
                fiat_amount: Some(10.0),
                fiat_amount_min: None,
                fiat_amount_max: None,
                fiat_code: "USD".into(),
                payment_method: "cash".into(),
                premium: 0.0,
                creator_pubkey: String::new(),
                created_at: 1,
                expires_at: None,
                is_mine: false,
                rating: 0.0,
                total_reviews: 0,
                days_active: 0,
                maker_since: None,
                cashu_mint_url: None,
            },
            role: TradeRole::Buyer,
            counterparty_pubkey: String::new(),
            current_step: TradeStep::Buyer(BuyerStep::OrderTaken),
            hold_invoice: None,
            buyer_invoice: None,
            trade_key_index: 1,
            cooperative_cancel_state: None,
            timeout_at: None,
            started_at: 1,
            completed_at: None,
            escrow_locked_at: None,
            escrow_node: None,
            outcome: None,
            peer_rating: None,
            peer_reviews: None,
            peer_days: None,
            peer_since: None,
            rated_at: None,
            bond: None,
            buyer_trade_pubkey: None,
            seller_trade_pubkey: None,
            cashu_mint_url: None,
            cashu_escrow_token: None,
            cashu_locked_at: None,
            cashu_rejected_escrow_tokens: Vec::new(),
        }
    }

    fn tags(pairs: &[(&str, &str)]) -> Vec<Vec<String>> {
        pairs
            .iter()
            .map(|(name, value)| vec![name.to_string(), value.to_string()])
            .collect()
    }

    #[test]
    fn the_guardian_watches_active_and_fiat_sent_only() {
        for status in [OrderStatus::Active, OrderStatus::FiatSent] {
            let mut t = trade("o1", status);
            t.escrow_locked_at = Some(1_000);
            t.escrow_node = Some("node".into());
            assert_eq!(escrow_lock_of(&t), Some((1_000, "node")));
        }
        for status in [
            OrderStatus::WaitingPayment,
            OrderStatus::WaitingBuyerInvoice,
            OrderStatus::Dispute,
            OrderStatus::SettledHoldInvoice,
            OrderStatus::Canceled,
            OrderStatus::Success,
        ] {
            let mut t = trade("o1", status.clone());
            t.escrow_locked_at = Some(1_000);
            t.escrow_node = Some("node".into());
            assert_eq!(escrow_lock_of(&t), None, "{status:?}");
        }
    }

    #[test]
    fn no_lock_time_means_no_deadline() {
        let mut t = trade("o1", OrderStatus::Active);
        assert_eq!(escrow_lock_of(&t), None);
        t.escrow_locked_at = Some(1_000);
        assert_eq!(escrow_lock_of(&t), None, "no node to read the window from");
    }

    /// The deadline is the lock time plus the window of the node that sent
    /// the lock, not the active node's.
    #[tokio::test]
    async fn the_deadline_counts_the_lock_nodes_window_from_the_lock() {
        let path =
            std::env::temp_dir().join(format!("mostro_escrow_deadline_{}.db", std::process::id()));
        let _ = crate::db::app_db::init_db(path.to_str().unwrap()).await;
        let db = crate::db::app_db::db().expect("store initialised");

        let node = "bb".repeat(32);
        crate::mostro::node_info::set_from_tags(
            &node,
            &tags(&[
                ("hold_invoice_cltv_delta", "144"),
                ("escrow_deadline_margin_blocks", "24"),
            ]),
        );
        let order_id = uuid::Uuid::new_v4().to_string();
        db.save_trade(&trade(&order_id, OrderStatus::Active))
            .await
            .unwrap();
        assert_eq!(trade_escrow_deadline(order_id.clone()).await, None);

        db.mark_escrow_locked(&order_id, 1_000, &node)
            .await
            .unwrap();
        assert_eq!(
            trade_escrow_deadline(order_id.clone()).await,
            Some(1_000 + 120 * 600)
        );

        // A node this app knows nothing about gives no deadline.
        let unknown = uuid::Uuid::new_v4().to_string();
        db.save_trade(&trade(&unknown, OrderStatus::FiatSent))
            .await
            .unwrap();
        db.mark_escrow_locked(&unknown, 1_000, &"cc".repeat(32))
            .await
            .unwrap();
        assert_eq!(trade_escrow_deadline(unknown).await, None);
    }
}
