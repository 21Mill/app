//! Shared `mostro_core` fixtures for the protocol modules' unit tests.
//!
//! `SmallOrder::new` takes fifteen positional arguments, only two of which
//! any of these tests care about. Both `pending` (classifying a take reply)
//! and `status` (syncing an add-invoice) need one, so the constructor lives
//! here instead of being spelled out — differently — in each test module.
//! The same goes for a `TradeInfo` row, which `push` and `trade_node` share.

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
