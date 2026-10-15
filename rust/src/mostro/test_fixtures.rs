//! Shared `mostro_core` fixtures for the protocol modules' unit tests.
//!
//! `SmallOrder::new` takes fifteen positional arguments, only two of which
//! any of these tests care about. Both `pending` (classifying a take reply)
//! and `status` (syncing an add-invoice) need one, so the constructor lives
//! here instead of being spelled out — differently — in each test module.
//! The same goes for a `TradeInfo` row, which `push` and `trade_node` share,
//! and for storing one where the trade actions look it up.

/// A `SmallOrder` carrying just `status` and `amount`; every other field is
/// the same inert placeholder for all callers.
pub(crate) fn small_order_with(
    status: mostro_core::order::Status,
    amount: i64,
) -> mostro_core::order::SmallOrder {
    mostro_core::order::SmallOrder::new(
        None,
        Some(mostro_core::order::Kind::Sell),
        Some(status),
        amount,
        "USD".to_string(),
        None,
        None,
        100,
        "bank".to_string(),
        0,
        None,
        None,
        None,
        None,
        None,
    )
}

/// A `TradeInfo` row for order `id` under trade-key `index`, in `status`,
/// issued by `node`; every other field is an inert placeholder.
pub(crate) fn trade(
    id: &str,
    index: u32,
    status: crate::api::types::OrderStatus,
    node: &str,
) -> crate::api::types::TradeInfo {
    use crate::api::types::{OrderKind, TradeInfo, TradeRole, TradeStep};
    TradeInfo {
        id: id.to_string(),
        order: crate::api::types::OrderInfo {
            id: id.to_string(),
            kind: OrderKind::Sell,
            status,
            amount_sats: None,
            fiat_amount: None,
            fiat_amount_min: None,
            fiat_amount_max: None,
            fiat_code: "USD".into(),
            payment_method: "x".into(),
            premium: 0.0,
            node_pubkey: node.to_string(),
            created_at: 0,
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
        current_step: TradeStep::Disputed,
        hold_invoice: None,
        buyer_invoice: None,
        trade_key_index: index,
        cooperative_cancel_state: None,
        timeout_at: None,
        started_at: 0,
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

/// The nodes a trade action must refuse while the default node is active.
/// A row with none recorded is not one: it goes to the active node.
pub(crate) fn nodes_other_than_the_active_one() -> [String; 1] {
    [nostr_sdk::prelude::Keys::generate().public_key().to_hex()]
}

/// Store `row` in the app database with its trade-key binding, as a trade
/// action finds it.
pub(crate) async fn store_trade_row(row: &crate::api::types::TradeInfo) {
    use crate::db::Storage as _;
    let path = std::env::temp_dir().join(format!("mostro_trade_rows_{}.db", std::process::id()));
    let _ = crate::db::app_db::init_db(path.to_str().unwrap()).await;
    let db = crate::db::app_db::db().expect("store initialised");
    db.save_trade(row).await.unwrap();
    db.save_trade_key(&row.order.id, row.trade_key_index)
        .await
        .unwrap();
}

/// Remove what [`store_trade_row`] stored, so no other test sees the row.
pub(crate) async fn forget_trade_row(order_id: &str) {
    use crate::db::Storage as _;
    let db = crate::db::app_db::db().expect("store initialised");
    db.delete_trade_by_order_id(order_id).await.unwrap();
    db.delete_trade_key(order_id).await.unwrap();
}
