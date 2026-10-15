//! Where the buyer's payout was sent, and what became of each destination
//! (#823).
//!
//! The node never says where it paid: `purchase-completed` carries nothing.
//! So this device keeps its own record of what it submitted — the Lightning
//! address sent with the take, each invoice or address sent afterwards, with
//! how it was chosen — and marks each one from the node's payout messages:
//!
//! | message | applies to the latest open destination |
//! |---|---|
//! | `payment-failed` (first failure only) | `Retrying`, with the node's attempt count |
//! | `add-invoice` on a settled escrow | `Failed`: the node gave up and wants another |
//! | `purchase-completed` | `Paid` |
//!
//! "Open" is `Submitted` or `Retrying`; `Failed`, `Rejected` and `Paid` never
//! change again, so a replayed message finds nothing left to mark. The
//! callers also apply the status cursor first, so a message older than one
//! already applied never reaches here.
//!
//! Stored as JSON under `payout_route:<order_id>`, scoped by the trade key
//! index like the payout amount: a later take starts a new record, and the
//! key goes with the row (`wipe_trade_row`, `clear_maker_step_start`) and
//! with the identity (`IDENTITY_SCOPED_PREFIXES`).

use std::sync::OnceLock;

use crate::api::types::{PayoutDestination, PayoutOutcome, PayoutSource, TakePayout};
use crate::db::Storage;

/// The stored record of one order's payout.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PayoutRoute {
    /// The trade key the destinations belong to.
    pub trade_index: u32,
    /// What the take asked for. `None` when the record did not start at a
    /// take on this device: a maker-buyer gives every destination through
    /// `add-invoice`.
    #[serde(default)]
    pub take_payout: Option<TakePayout>,
    /// Oldest first.
    #[serde(default)]
    pub destinations: Vec<PayoutDestination>,
}

/// What a node message says about the payout in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PayoutNews {
    /// `payment-failed`: the first payment failed and the node retries,
    /// trying `attempts` times in all when it says how many.
    Retrying { attempts: Option<u32> },
    /// `add-invoice` on a settled escrow: the node gave up on it.
    Failed,
    /// `purchase-completed`: the node paid it. `amount_sats` fills the
    /// amount of a destination that did not carry one (an address).
    Paid { amount_sats: Option<u64> },
}

/// A destination as sent: what it is, how it was chosen, and what the
/// invoice itself says when it is one.
pub(crate) fn destination(
    input: &str,
    source: PayoutSource,
    wallet_name: Option<String>,
    submitted_at: i64,
    amount_hint: Option<u64>,
) -> PayoutDestination {
    let destination = crate::api::invoice::normalize(input).to_string();
    let invoice = crate::api::invoice::bolt11_facts(&destination);
    let is_lightning_address = matches!(
        crate::api::invoice::classify(&destination),
        crate::api::types::PaymentDestination::LightningAddress(_)
    );
    PayoutDestination {
        destination,
        is_lightning_address,
        source,
        wallet_name: wallet_name.filter(|name| !name.trim().is_empty()),
        submitted_at,
        amount_sats: invoice
            .as_ref()
            .and_then(|facts| facts.amount_sats)
            .or(amount_hint),
        payment_hash: invoice.map(|facts| facts.payment_hash),
        outcome: PayoutOutcome::Submitted,
        outcome_at: None,
        attempts: None,
    }
}

/// The record after the take on trade key `trade_index` asked for
/// `take_payout`, sending `sent` when it named an address. `None` when the
/// stored record belongs to a later take and must stand.
pub(crate) fn with_take(
    stored: Option<PayoutRoute>,
    trade_index: u32,
    take_payout: TakePayout,
    sent: Option<PayoutDestination>,
) -> Option<PayoutRoute> {
    if stored.as_ref().is_some_and(|r| r.trade_index > trade_index) {
        return None;
    }
    Some(PayoutRoute {
        trade_index,
        take_payout: Some(take_payout),
        destinations: sent.into_iter().collect(),
    })
}

/// The record after `sent` went out on trade key `trade_index`. A record of
/// an earlier take is replaced; `None` when the stored one belongs to a
/// later take.
pub(crate) fn with_destination(
    stored: Option<PayoutRoute>,
    trade_index: u32,
    sent: PayoutDestination,
) -> Option<PayoutRoute> {
    let mut route = match stored {
        Some(route) if route.trade_index > trade_index => return None,
        Some(route) if route.trade_index == trade_index => route,
        _ => PayoutRoute {
            trade_index,
            take_payout: None,
            destinations: Vec::new(),
        },
    };
    route.destinations.push(sent);
    Some(route)
}

/// The record after `news`, dated `at`, about the payout on trade key
/// `trade_index`, or `None` when it changes nothing: another trade's
/// message, no destination still open, or a step the open one already took.
pub(crate) fn with_news(
    stored: Option<PayoutRoute>,
    trade_index: u32,
    news: PayoutNews,
    at: i64,
) -> Option<PayoutRoute> {
    let mut route = stored.filter(|r| r.trade_index == trade_index)?;
    let open = route.destinations.iter_mut().rev().find(|d| {
        matches!(
            d.outcome,
            PayoutOutcome::Submitted | PayoutOutcome::Retrying
        )
    })?;
    match news {
        PayoutNews::Retrying { attempts } => {
            // The node says it once, at the first failure.
            if open.outcome != PayoutOutcome::Submitted {
                return None;
            }
            open.outcome = PayoutOutcome::Retrying;
            open.attempts = attempts;
        }
        PayoutNews::Failed => open.outcome = PayoutOutcome::Failed,
        PayoutNews::Paid { amount_sats } => {
            open.outcome = PayoutOutcome::Paid;
            if open.amount_sats.is_none() {
                open.amount_sats = amount_sats;
            }
        }
    }
    open.outcome_at = Some(at);
    Some(route)
}

/// The destinations of the trade the row is on now: none when the record
/// belongs to another take. An unknown row index is not a mismatch.
pub(crate) fn destinations_for(
    stored: Option<PayoutRoute>,
    row_index: Option<u32>,
) -> Vec<PayoutDestination> {
    match stored {
        Some(route) if row_index.is_none_or(|index| index == route.trade_index) => {
            route.destinations
        }
        _ => Vec::new(),
    }
}

/// The address the take on trade key `trade_index` sent, or `Some(None)`
/// when it sent none; `None` when no take of that key was recorded here.
pub(crate) fn take_address_for(stored: &PayoutRoute, trade_index: u32) -> Option<Option<String>> {
    if stored.trade_index != trade_index {
        return None;
    }
    let take = stored.take_payout.as_ref()?;
    Some(match take {
        TakePayout::AskForInvoice => None,
        TakePayout::DefaultAddress | TakePayout::Address { .. } => stored
            .destinations
            .first()
            .filter(|d| d.is_lightning_address)
            .map(|d| d.destination.clone()),
    })
}

// ── Persistence ─────────────────────────────────────────────────────────────

/// Serializes the read-modify-write of every record: the dispatcher holds
/// the order's lock, but `send_invoice` and `take_order` write outside it.
fn write_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn load(db: &impl Storage, order_id: &str) -> Option<PayoutRoute> {
    let key = crate::db::settings_keys::payout_route(order_id);
    let raw = db.get_setting(&key).await.ok().flatten()?;
    match serde_json::from_str(&raw) {
        Ok(route) => Some(route),
        Err(e) => {
            log::warn!("[payout] unreadable record for order={order_id}: {e}");
            None
        }
    }
}

/// Load, change and store `order_id`'s record under the write lock. Returns
/// whether it was written.
async fn update(
    db: &impl Storage,
    order_id: &str,
    change: impl FnOnce(Option<PayoutRoute>) -> Option<PayoutRoute>,
) -> bool {
    let _guard = write_lock().lock().await;
    let Some(route) = change(load(db, order_id).await) else {
        return false;
    };
    let value = match serde_json::to_string(&route) {
        Ok(value) => value,
        Err(e) => {
            log::warn!("[payout] record not encoded for order={order_id}: {e}");
            return false;
        }
    };
    let key = crate::db::settings_keys::payout_route(order_id);
    match db.set_setting(&key, &value).await {
        Ok(()) => true,
        Err(e) => {
            crate::api::logging::blog_warn(
                "payout",
                format!(
                    "record not persisted for order={}: {e}",
                    crate::api::logging::short_id(order_id),
                ),
            );
            false
        }
    }
}

/// Record what the take on `trade_index` asked for.
pub(crate) async fn record_take_in(
    db: &impl Storage,
    order_id: &str,
    trade_index: u32,
    take_payout: TakePayout,
    sent: Option<PayoutDestination>,
) -> bool {
    update(db, order_id, |stored| {
        with_take(stored, trade_index, take_payout, sent)
    })
    .await
}

/// Record a destination sent on `trade_index`.
pub(crate) async fn record_destination_in(
    db: &impl Storage,
    order_id: &str,
    trade_index: u32,
    sent: PayoutDestination,
) -> bool {
    update(db, order_id, |stored| {
        with_destination(stored, trade_index, sent)
    })
    .await
}

/// Apply a node's payout `news` about `trade_index`, dated `at`.
pub(crate) async fn record_news_in(
    db: &impl Storage,
    order_id: &str,
    trade_index: u32,
    news: PayoutNews,
    at: i64,
) -> bool {
    update(db, order_id, |stored| {
        with_news(stored, trade_index, news, at)
    })
    .await
}

/// Drop the record of the take on `trade_index`, when it is still that
/// take's: the node refused the take, so nothing was sent anywhere.
pub(crate) async fn forget_take_in(db: &impl Storage, order_id: &str, trade_index: u32) {
    let _guard = write_lock().lock().await;
    if load(db, order_id)
        .await
        .is_some_and(|route| route.trade_index == trade_index)
    {
        forget_unlocked(db, order_id).await;
    }
}

/// Drop `order_id`'s record whoever wrote it: the row it describes is gone.
pub(crate) async fn forget_in(db: &impl Storage, order_id: &str) {
    let _guard = write_lock().lock().await;
    forget_unlocked(db, order_id).await;
}

async fn forget_unlocked(db: &impl Storage, order_id: &str) {
    let key = crate::db::settings_keys::payout_route(order_id);
    if let Err(e) = db.delete_setting(&key).await {
        crate::api::logging::blog_warn(
            "payout",
            format!(
                "record not cleared for order={}: {e}",
                crate::api::logging::short_id(order_id),
            ),
        );
    }
}

/// [`record_take_in`] on the app's store; rings the trade's doorbell when
/// written.
pub(crate) async fn record_take(
    order_id: &str,
    trade_index: u32,
    take_payout: TakePayout,
    sent: Option<PayoutDestination>,
) {
    let Some(db) = crate::db::app_db::db() else {
        return;
    };
    if record_take_in(db, order_id, trade_index, take_payout, sent).await {
        crate::api::trade_touch::touch_trade(order_id);
    }
}

/// [`record_destination_in`] on the app's store; rings the doorbell.
pub(crate) async fn record_destination(order_id: &str, trade_index: u32, sent: PayoutDestination) {
    let Some(db) = crate::db::app_db::db() else {
        return;
    };
    if record_destination_in(db, order_id, trade_index, sent).await {
        crate::api::trade_touch::touch_trade(order_id);
    }
}

/// [`record_news_in`] on the app's store; rings the doorbell.
pub(crate) async fn record_news(order_id: &str, trade_index: u32, news: PayoutNews, at: i64) {
    let Some(db) = crate::db::app_db::db() else {
        return;
    };
    if record_news_in(db, order_id, trade_index, news, at).await {
        crate::api::trade_touch::touch_trade(order_id);
    }
}

/// [`forget_take_in`] on the app's store.
pub(crate) async fn forget_take(order_id: &str, trade_index: u32) {
    if let Some(db) = crate::db::app_db::db() {
        forget_take_in(db, order_id, trade_index).await;
    }
}

/// The destinations of the trade `order_id`'s row is on now, oldest first.
pub(crate) async fn destinations(order_id: &str) -> Vec<PayoutDestination> {
    let Some(db) = crate::db::app_db::db() else {
        return Vec::new();
    };
    let row_index = db
        .get_trade_by_order_id(order_id)
        .await
        .ok()
        .flatten()
        .map(|trade| trade.trade_key_index);
    destinations_for(load(db, order_id).await, row_index)
}

/// What the take on `trade_index` sent, when this device recorded it: the
/// bond re-request repeats it instead of reading Settings again.
pub(crate) async fn take_address(order_id: &str, trade_index: u32) -> Option<Option<String>> {
    let db = crate::db::app_db::db()?;
    take_address_for(&load(db, order_id).await?, trade_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::invoice::test_vectors::COFFEE;

    const ADDRESS: &str = "alice@example.com";

    fn sent(input: &str, source: PayoutSource, at: i64) -> PayoutDestination {
        destination(input, source, None, at, None)
    }

    fn outcomes(route: &PayoutRoute) -> Vec<PayoutOutcome> {
        route.destinations.iter().map(|d| d.outcome).collect()
    }

    /// A route that starts with the default address sent at take.
    fn take_route(index: u32) -> PayoutRoute {
        with_take(
            None,
            index,
            TakePayout::DefaultAddress,
            Some(sent(ADDRESS, PayoutSource::DefaultLightningAddress, 100)),
        )
        .unwrap()
    }

    #[test]
    fn an_address_is_recorded_as_one_without_a_hash() {
        let d = destination(
            " Alice@Example.com ",
            PayoutSource::Entered,
            None,
            5,
            Some(311),
        );

        assert_eq!(d.destination, "Alice@Example.com");
        assert!(d.is_lightning_address);
        assert_eq!(d.payment_hash, None);
        assert_eq!(d.amount_sats, Some(311));
        assert_eq!(d.outcome, PayoutOutcome::Submitted);
    }

    #[test]
    fn an_invoice_is_recorded_with_its_hash_and_amount() {
        let d = destination(
            &format!("lightning:{COFFEE}"),
            PayoutSource::NwcWallet,
            Some("Alby".into()),
            5,
            Some(1),
        );

        assert!(!d.is_lightning_address);
        assert_eq!(d.destination, COFFEE);
        assert_eq!(
            d.payment_hash.as_deref(),
            Some("0001020304050607080900010203040506070809000102030405060708090102")
        );
        // The invoice's own amount, not the hint.
        assert_eq!(d.amount_sats, Some(250_000));
        assert_eq!(d.wallet_name.as_deref(), Some("Alby"));
    }

    #[test]
    fn a_blank_wallet_name_is_no_name() {
        let d = destination(COFFEE, PayoutSource::NwcWallet, Some("  ".into()), 5, None);
        assert_eq!(d.wallet_name, None);
    }

    #[test]
    fn a_later_take_replaces_the_record_and_an_earlier_one_cannot() {
        let route = take_route(7);

        let retake = with_take(Some(route.clone()), 8, TakePayout::AskForInvoice, None).unwrap();
        assert_eq!(retake.trade_index, 8);
        assert!(retake.destinations.is_empty());

        assert_eq!(
            with_take(Some(retake), 7, TakePayout::DefaultAddress, None),
            None
        );
    }

    #[test]
    fn destinations_accumulate_within_one_trade() {
        let route = with_destination(
            Some(take_route(7)),
            7,
            sent(COFFEE, PayoutSource::NwcWallet, 200),
        )
        .unwrap();

        assert_eq!(route.destinations.len(), 2);
        assert_eq!(route.take_payout, Some(TakePayout::DefaultAddress));
    }

    #[test]
    fn a_destination_for_an_older_key_is_dropped() {
        assert_eq!(
            with_destination(
                Some(take_route(7)),
                6,
                sent(COFFEE, PayoutSource::Entered, 9)
            ),
            None
        );
    }

    #[test]
    fn a_failed_address_then_a_paid_invoice() {
        // The staging trade behind #823: the default address fails three
        // times, the NWC wallet's invoice is paid.
        let route = take_route(7);
        let route = with_news(
            Some(route),
            7,
            PayoutNews::Retrying { attempts: Some(3) },
            110,
        )
        .unwrap();
        let route = with_news(Some(route), 7, PayoutNews::Failed, 220).unwrap();
        let route =
            with_destination(Some(route), 7, sent(COFFEE, PayoutSource::NwcWallet, 221)).unwrap();
        let route = with_news(Some(route), 7, PayoutNews::Paid { amount_sats: None }, 280).unwrap();

        assert_eq!(
            outcomes(&route),
            [PayoutOutcome::Failed, PayoutOutcome::Paid]
        );
        assert_eq!(route.destinations[0].attempts, Some(3));
        assert_eq!(route.destinations[0].outcome_at, Some(220));
        assert_eq!(route.destinations[1].outcome_at, Some(280));
    }

    #[test]
    fn a_settled_destination_is_never_marked_again() {
        let route = with_news(
            Some(take_route(7)),
            7,
            PayoutNews::Paid {
                amount_sats: Some(311),
            },
            300,
        )
        .unwrap();
        assert_eq!(route.destinations[0].amount_sats, Some(311));

        // A replay of any earlier news finds nothing open.
        for news in [
            PayoutNews::Retrying { attempts: Some(3) },
            PayoutNews::Failed,
            PayoutNews::Paid { amount_sats: None },
        ] {
            assert_eq!(with_news(Some(route.clone()), 7, news, 100), None);
        }
    }

    #[test]
    fn the_retry_notice_only_opens_a_retry_once() {
        let route = with_news(
            Some(take_route(7)),
            7,
            PayoutNews::Retrying { attempts: Some(3) },
            110,
        )
        .unwrap();

        assert_eq!(
            with_news(
                Some(route),
                7,
                PayoutNews::Retrying { attempts: Some(5) },
                120
            ),
            None
        );
    }

    #[test]
    fn news_for_another_trade_key_changes_nothing() {
        assert_eq!(
            with_news(Some(take_route(7)), 8, PayoutNews::Failed, 9),
            None
        );
        assert_eq!(with_news(None, 7, PayoutNews::Failed, 9), None);
    }

    #[test]
    fn a_rejected_destination_is_not_the_one_marked() {
        let mut refused = sent(COFFEE, PayoutSource::Entered, 150);
        refused.outcome = PayoutOutcome::Rejected;
        let route = with_destination(Some(take_route(7)), 7, refused).unwrap();

        let route = with_news(Some(route), 7, PayoutNews::Paid { amount_sats: None }, 300).unwrap();

        assert_eq!(
            outcomes(&route),
            [PayoutOutcome::Paid, PayoutOutcome::Rejected]
        );
    }

    #[test]
    fn destinations_show_only_for_the_row_they_belong_to() {
        let route = take_route(7);

        assert_eq!(destinations_for(Some(route.clone()), Some(7)).len(), 1);
        assert_eq!(destinations_for(Some(route.clone()), None).len(), 1);
        assert!(destinations_for(Some(route), Some(8)).is_empty());
        assert!(destinations_for(None, Some(7)).is_empty());
    }

    #[test]
    fn the_take_address_is_what_the_take_sent() {
        assert_eq!(
            take_address_for(&take_route(7), 7),
            Some(Some(ADDRESS.to_string()))
        );
        assert_eq!(take_address_for(&take_route(7), 8), None);

        let asked = with_take(None, 7, TakePayout::AskForInvoice, None).unwrap();
        assert_eq!(take_address_for(&asked, 7), Some(None));

        // A maker's record never started at a take.
        let maker = with_destination(None, 7, sent(COFFEE, PayoutSource::Entered, 1)).unwrap();
        assert_eq!(take_address_for(&maker, 7), None);
    }

    async fn temp_db() -> crate::db::sqlite::SqliteStorage {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("mostro_payout_route_{}_{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        crate::db::sqlite::SqliteStorage::open(path.to_str().unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_record_survives_a_restart_of_the_store() {
        let db = temp_db().await;
        let address = sent(ADDRESS, PayoutSource::DefaultLightningAddress, 100);
        assert!(record_take_in(&db, "order-1", 7, TakePayout::DefaultAddress, Some(address)).await);
        assert!(
            record_news_in(&db, "order-1", 7, PayoutNews::Failed, 200).await,
            "the address failed"
        );
        assert!(
            record_destination_in(
                &db,
                "order-1",
                7,
                sent(COFFEE, PayoutSource::NwcWallet, 201)
            )
            .await
        );
        // Nothing open is left to retry.
        assert!(
            !record_news_in(
                &db,
                "order-1",
                8,
                PayoutNews::Paid { amount_sats: None },
                300
            )
            .await,
            "another trade key's news"
        );

        let stored = load(&db, "order-1").await.unwrap();
        assert_eq!(
            outcomes(&stored),
            [PayoutOutcome::Failed, PayoutOutcome::Submitted]
        );
        assert_eq!(stored.destinations[1].source, PayoutSource::NwcWallet);
    }

    #[tokio::test]
    async fn a_refused_take_forgets_only_its_own_record() {
        let db = temp_db().await;
        record_take_in(&db, "order-2", 9, TakePayout::AskForInvoice, None).await;

        forget_take_in(&db, "order-2", 8).await;
        assert!(
            load(&db, "order-2").await.is_some(),
            "another take's refusal"
        );

        forget_take_in(&db, "order-2", 9).await;
        assert!(load(&db, "order-2").await.is_none());
    }

    #[tokio::test]
    async fn an_unreadable_record_is_treated_as_none() {
        let db = temp_db().await;
        db.set_setting(
            &crate::db::settings_keys::payout_route("order-3"),
            "{not json",
        )
        .await
        .unwrap();

        assert!(load(&db, "order-3").await.is_none());
        // And the next write replaces it.
        assert!(
            record_destination_in(&db, "order-3", 1, sent(COFFEE, PayoutSource::Entered, 1)).await
        );
        assert_eq!(load(&db, "order-3").await.unwrap().destinations.len(), 1);
    }
}
