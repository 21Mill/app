//! Late take replies (#566): a take whose daemon reply outlived the
//! caller's 10 s waiter still lands its trade while the app runs —
//! rebuilt from the take record's snapshot, persisted, bound, wired and
//! announced — with a bounded local retry when the store fails.
//!
//! The dispatcher hands a late reply to [`reconcile_late_take`] and settles
//! it with [`settle_late_take_retry`]; the identity teardown stops the
//! retries with [`forget_late_take_retries`]. Everything else is private.
//! Nothing here is `pub`: flutter_rust_bridge scans `crate::api`, and a
//! `pub` item would be bridged to Dart.

use super::*;

/// What [`reconcile_late_take`] did with a take reply that outlived its
/// waiter.
pub(super) enum LateTakeOutcome {
    /// The trade landed: persisted, bound, announced and wired up.
    Landed(Box<crate::api::types::TradeInfo>),
    /// The reply proves no row, and none stands in its way: the arms take
    /// it as any message about a trade with no row.
    Declined,
    /// The reply must not reach the arms: the only row they would find is
    /// an earlier take's, or the identity the take began under is gone.
    Drop,
    /// The store failed: the reply is scheduled for another attempt
    /// ([`retry_late_take`]) and must not reach the arms meanwhile.
    Retry,
}

/// A take reply that outlived its waiter (#566), with what reconciling it —
/// and retrying that — needs.
pub(super) struct LateTake {
    /// The take record the reply consumed.
    pub(super) pending: PendingRequest,
    /// The reply as it arrived, kept only for a retry the store may need.
    pub(super) original: mostro_core::transport::UnwrappedMessage,
    pub(super) order_id: String,
    pub(super) trade_pubkey_hex: String,
    pub(super) trade_index: u32,
    pub(super) event_id: String,
    pub(super) event_ts: i64,
    /// The teardown epoch the dispatch began under
    /// ([`late_take_retry_epoch`]).
    pub(super) epoch: u64,
}

/// Lands a take whose reply outlived its 10 s waiter (#566) — or leaves
/// everything as it was, never half of it.
///
/// Builds first and writes after. The row comes from the take record's
/// snapshot, as the live waiter would have built it ([`take_snapshot_row`]),
/// else from the message ([`dm_trade_row`]); only once one exists and the
/// status cursor admits the reply is anything written. A row an **earlier**
/// take of the order left behind (lower `trade_key_index` — its `Canceled`
/// never reached us) is replaced by it in ONE storage transaction
/// ([`Storage::replace_trades_for_order`]): whatever fails, or if the
/// process dies halfway, the order keeps that earlier row. What this refuses
/// to do is apply the reply to that row, whose role and key are another
/// trade's: a reply that proves no row of its own is dropped with the
/// earlier row left in place.
///
/// A store failure is retried here, not left to a redelivery: nostr-sdk
/// never notifies an event id its client already saw again (the client's
/// shared events tracker, `relay/inner.rs`), so no relay copy, reconnect or
/// resync brings the same reply back within the session.
/// [`retry_late_take`] keeps the reply and its take record and dispatches
/// them again on a short backoff. For a reply with no order payload the
/// record is the only source of the row there is. Nothing is announced and
/// nothing is bound before the row is durable.
pub(super) async fn reconcile_late_take(
    late: LateTake,
    kind: &mostro_core::message::MessageKind,
    row_state: &RowState,
) -> LateTakeOutcome {
    let retry = |late: LateTake, why: &str| {
        retry_late_take(late, why);
        LateTakeOutcome::Retry
    };
    let order_id = late.order_id.as_str();
    let trade_pubkey_hex = late.trade_pubkey_hex.as_str();
    let (trade_index, event_ts) = (late.trade_index, late.event_ts);
    let earlier = match row_state {
        RowState::NeverWritten => None,
        RowState::Exists(row) if row.trade_key_index < late.pending.trade_index => {
            Some(row.as_ref())
        }
        // This take's own row already stands, or the order was wiped on
        // purpose: the arms and their gates own the message.
        RowState::Exists(_) | RowState::Wiped => return LateTakeOutcome::Declined,
        // The store could not say what stands: nothing may be decided.
        RowState::Unknown => return retry(late, "store unavailable"),
    };
    // Admission. Over an earlier take's row only the ordering half of the
    // status gate speaks: the terminal half would read that row's status as
    // this take's and refuse a reply that ends nothing.
    let blocked = match earlier {
        Some(_) => status_cursor_blocks(order_id, &kind.action, event_ts).await,
        None => status_write_blocked(order_id, &kind.action, event_ts).await,
    };
    let candidate = if blocked {
        None
    } else {
        match take_snapshot_row(&late.pending, kind, order_id, event_ts) {
            Some(trade) => Some((trade, "take record", "#566")),
            None => dm_trade_row(
                kind,
                order_id,
                trade_pubkey_hex,
                &late.original.sender.to_hex(),
                trade_index,
                event_ts,
            )
            .map(|trade| (trade, "DM", "#394")),
        }
    };
    let Some((mut trade, source, issue)) = candidate else {
        if let Some(row) = earlier {
            crate::api::logging::blog_info(
                "orders",
                format!(
                    "late take order={} at trade_index={} proves no row to replace the one of \
                     trade_index={} with — dropped, that row kept (#566)",
                    crate::api::logging::short_id(order_id),
                    late.pending.trade_index,
                    row.trade_key_index,
                ),
            );
            return LateTakeOutcome::Drop;
        }
        return LateTakeOutcome::Declined;
    };
    let Some(db) = crate::db::app_db::db() else {
        return retry(late, "store unavailable");
    };
    #[cfg(test)]
    late_take_test_pause("before-write").await;
    // A first attempt is no retry task, so the identity teardown cannot stop
    // it: it checks the epoch itself, before its write and again after it,
    // so nothing past either point runs for a forgotten identity. A write
    // still in flight when the teardown wipes the rows can outlive the wipe;
    // nothing bound, announced or wired follows it.
    if late_take_identity_gone(&late, "before its write") {
        return LateTakeOutcome::Drop;
    }
    let written = match earlier {
        Some(_) => replace_with_late_take_row(db, &trade).await,
        None => save_late_take_row(db, &trade).await,
    };
    if let Err(e) = written {
        return retry(late, &format!("row not persisted ({e})"));
    }
    #[cfg(test)]
    late_take_test_pause("after-write").await;
    if late_take_identity_gone(&late, "after its write") {
        return LateTakeOutcome::Drop;
    }
    if let Some(row) = earlier {
        crate::api::logging::blog_info(
            "orders",
            format!(
                "late take order={} at trade_index={} superseded the row of trade_index={} (#566)",
                crate::api::logging::short_id(order_id),
                late.pending.trade_index,
                row.trade_key_index,
            ),
        );
    }
    // Bound only once the row is durable, so a failed reconcile leaves the
    // binding where the stored row expects it.
    store_trade_key_index(order_id, trade.trade_key_index).await;
    let status = trade.order.status.clone();
    crate::api::logging::blog_info(
        "orders",
        format!(
            "rebuilt trade row from {source} order={} role={:?} status={status:?} \
             trade_index={} src=kind14/{:?} ({issue})",
            crate::api::logging::short_id(order_id),
            trade.role,
            trade.trade_key_index,
            kind.action,
        ),
    );
    emit_trade_update_at(order_id, status, None, event_ts);
    // A late take lands wired up like one that landed in time: watched,
    // with its session installed.
    wire_accepted_take(&mut trade, "late take").await;
    LateTakeOutcome::Landed(Box::new(trade))
}

/// Whether the identity `late`'s dispatch began under has been torn down
/// since: the teardown bumps the epoch before it clears anything
/// ([`forget_late_take_retries`]). Logged when it has, `at` saying where the
/// take was dropped.
fn late_take_identity_gone(late: &LateTake, at: &str) -> bool {
    let gone = late.epoch != late_take_retry_epoch();
    if gone {
        crate::api::logging::blog_info(
            "orders",
            format!(
                "late take order={} dropped {at}: its identity is gone (#566)",
                crate::api::logging::short_id(&late.order_id),
            ),
        );
    }
    gone
}

/// How long a late take the store failed to land waits before each further
/// attempt; one attempt per entry, then it is given up.
#[cfg(not(test))]
const LATE_TAKE_RETRY_DELAYS: [std::time::Duration; 3] = [
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(30),
    std::time::Duration::from_secs(120),
];
#[cfg(test)]
const LATE_TAKE_RETRY_DELAYS: [std::time::Duration; 3] = [
    std::time::Duration::from_millis(5),
    std::time::Duration::from_millis(5),
    std::time::Duration::from_millis(5),
];

/// A scheduled retry's task, as the identity teardown needs it: to stop it
/// and to know it has stopped.
#[flutter_rust_bridge::frb(ignore)]
struct LateTakeRetryTask {
    abort: futures_util::future::AbortHandle,
    /// Resolves (with an error) once the task's future is gone — finished or
    /// aborted — because the task holds the sender until then.
    stopped: tokio::sync::oneshot::Receiver<()>,
}

/// The late-take retries (#566), in two parts with two lifetimes.
///
/// `frb(ignore)` for the reason `DedupWindow` gives: its `Default` would
/// otherwise make the codegen bridge this private type.
#[derive(Default)]
#[flutter_rust_bridge::frb(ignore)]
struct LateTakeRetries {
    /// Attempts scheduled per late take, by event id: the retry bookkeeping,
    /// which ends when the take is settled — landed, or proven to have
    /// nothing to land.
    attempts: HashMap<String, usize>,
    /// Every retry task still running, by task id. Kept apart from the
    /// bookkeeping on purpose: a take is settled INSIDE its task's dispatch,
    /// whose action arm still writes afterwards, so the teardown must be
    /// able to stop the task until the task itself has finished.
    tasks: HashMap<u64, LateTakeRetryTask>,
    next_task: u64,
    /// Bumped by every identity teardown ([`forget_late_take_retries`]): a
    /// dispatch that began under an older epoch belongs to a forgotten
    /// identity and may not schedule a retry.
    epoch: u64,
}

/// What [`LateTakeRetries::schedule`] decided.
enum Scheduled {
    /// Attempt number `attempt` (from 0) is due, its task registered as
    /// `task_id`.
    Attempt { attempt: usize, task_id: u64 },
    /// The dispatch began under an epoch a teardown has since closed.
    IdentityGone,
    /// Every delay is spent: `attempts` were made.
    Exhausted { attempts: usize },
}

impl LateTakeRetries {
    /// Counts another attempt at the late take `event_id` and registers
    /// `task` to run it — or refuses, for a dispatch that began under a
    /// closed `epoch` or a take past its last delay, and then `task` is
    /// dropped unregistered. `put_back` returns the take record to the
    /// registry the interception reads; it runs only for a due attempt, and
    /// inside the same critical section as the check, the count and the
    /// registration, which the teardown's cannot interleave.
    fn schedule(
        &mut self,
        epoch: u64,
        event_id: &str,
        task: LateTakeRetryTask,
        put_back: impl FnOnce(),
    ) -> Scheduled {
        if self.epoch != epoch {
            return Scheduled::IdentityGone;
        }
        let attempts = self.attempts.entry(event_id.to_string()).or_insert(0);
        let attempt = *attempts;
        *attempts += 1;
        if attempt >= LATE_TAKE_RETRY_DELAYS.len() {
            self.attempts.remove(event_id);
            return Scheduled::Exhausted {
                attempts: attempt + 1,
            };
        }
        put_back();
        let task_id = self.next_task;
        self.next_task += 1;
        self.tasks.insert(task_id, task);
        Scheduled::Attempt { attempt, task_id }
    }

    /// Ends the bookkeeping of the late take `event_id`. Its task, if one is
    /// running, stays registered — and stoppable — until it finishes.
    fn settle(&mut self, event_id: &str) {
        self.attempts.remove(event_id);
    }

    /// The registry's half of an identity teardown: a new epoch, no
    /// bookkeeping, and every registered task handed over to be stopped
    /// ([`stop_retry_tasks`]).
    fn begin_forget(&mut self) -> Vec<LateTakeRetryTask> {
        // A first attempt already running when the identity goes is no task
        // of ours to stop: the new epoch refuses the retry it may schedule.
        self.epoch += 1;
        self.attempts.clear();
        self.tasks.drain().map(|(_, task)| task).collect()
    }
}

/// The registry the late-take retries live in: a mutex, because a retry
/// task and the dispatcher may run on different runtime threads.
type RetryRegistry = std::sync::Mutex<LateTakeRetries>;

/// `registry`, locked. A poisoned lock is taken as is: every critical
/// section leaves the registry consistent before anything in it can panic.
fn lock_retries(registry: &RetryRegistry) -> std::sync::MutexGuard<'_, LateTakeRetries> {
    registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The late-take retries. Identity-scoped: [`forget_late_take_retries`]
/// stops them when the identity goes.
///
/// Process-wide in the app. Under test it is one per thread instead — the
/// same mutex, leaked once per thread: a `#[tokio::test]` runs its retries
/// on its own thread, and the identity-lifecycle tests running alongside
/// tear the registry down, so a shared one would stop other tests' retries.
fn late_take_retries() -> &'static RetryRegistry {
    #[cfg(not(test))]
    {
        static RETRIES: OnceLock<RetryRegistry> = OnceLock::new();
        RETRIES.get_or_init(Default::default)
    }
    #[cfg(test)]
    {
        thread_local! {
            static RETRIES: &'static RetryRegistry = Box::leak(Box::default());
        }
        RETRIES.with(|retries| *retries)
    }
}

/// Runs `f` on the late-take retries, locked.
fn with_late_take_retries<R>(f: impl FnOnce(&mut LateTakeRetries) -> R) -> R {
    f(&mut lock_retries(late_take_retries()))
}

/// The current late-take teardown epoch, read by a dispatch as it begins so
/// a retry it schedules later can be refused if the identity went meanwhile.
pub(super) fn late_take_retry_epoch() -> u64 {
    with_late_take_retries(|retries| retries.epoch)
}

/// Stops every late-take retry task still running and waits until each has
/// stopped: they were the forgotten identity's (#533). Settled or not — a
/// retry that already landed its take is still inside its dispatch, whose
/// action arm writes the order's status cursor, step start and book entry.
///
/// Aborted, not merely unregistered: a retry already past its start would
/// otherwise run on — through the dispatch's awaits — and land the old
/// identity's trade, key binding, session and order state in the next
/// identity's.
/// An aborted task is never polled again, and the wait means none is still
/// running once this returns. So the identity teardown calls this FIRST:
/// whatever a retry wrote before it stopped, the rest of the teardown
/// removes, and nothing is written after.
pub(super) async fn forget_late_take_retries() {
    let tasks = with_late_take_retries(LateTakeRetries::begin_forget);
    stop_retry_tasks(tasks).await;
}

/// Aborts `tasks`, then waits until each one's future is gone. The wait is
/// what matters: an abort only takes effect at a task's next poll, so a
/// task in the middle of one — on another runtime thread — runs on until
/// that poll returns.
async fn stop_retry_tasks(tasks: Vec<LateTakeRetryTask>) {
    for task in &tasks {
        task.abort.abort();
    }
    for task in tasks {
        // `Err` is the answer: the sender went with the task's future.
        let _ = task.stopped.await;
    }
}

/// Ends the retry bookkeeping of a late take that no longer needs one — it
/// landed, or it proved nothing to land. Only the bookkeeping: the task
/// that settled it keeps running its dispatch and stays stoppable until it
/// finishes.
pub(super) fn settle_late_take_retry(event_id: &str) {
    with_late_take_retries(|retries| retries.settle(event_id));
}

/// Counts another attempt at the late take `event_id` in `registry` and, if
/// one is due, spawns the future `attempt` builds for it as the registered
/// retry task: abortable through the handle the registry holds, holding
/// that task's `stopped` sender until its future is gone — finished or
/// aborted — and taking itself out of `registry` when it finishes.
/// `put_back` runs inside the scheduling critical section
/// ([`LateTakeRetries::schedule`]); `attempt` runs after it, unlocked.
fn start_retry_task(
    registry: &'static RetryRegistry,
    epoch: u64,
    event_id: &str,
    put_back: impl FnOnce(),
    attempt: impl FnOnce(usize) -> RedispatchFuture<'static>,
) -> Scheduled {
    let (abort, registration) = futures_util::future::AbortHandle::new_pair();
    let (stopped_tx, stopped) = tokio::sync::oneshot::channel::<()>();
    let scheduled = lock_retries(registry).schedule(
        epoch,
        event_id,
        LateTakeRetryTask { abort, stopped },
        put_back,
    );
    if let Scheduled::Attempt {
        attempt: number,
        task_id,
    } = scheduled
    {
        let attempt = attempt(number);
        crate::rt::spawn(async move {
            // Dropped when this task ends — finished or aborted — which is
            // what `stop_retry_tasks` waits for.
            let _stopped = stopped_tx;
            let _ = futures_util::future::Abortable::new(attempt, registration).await;
            // Finished: nothing left to stop. (Aborted, the teardown already
            // took it out.)
            lock_retries(registry).tasks.remove(&task_id);
        });
    }
    scheduled
}

/// Schedules another attempt at a late take the store failed to land
/// (#566): the take record goes back into the registry, where the
/// interception finds it again, and the reply itself is kept and dispatched
/// again after the next backoff delay — through the whole dispatcher, per-
/// order lock and gates included. Past the last delay the take is given up
/// and logged; a restart's replay then rebuilds what the message itself
/// proves. The attempt runs as an abortable task the identity teardown
/// stops ([`forget_late_take_retries`]).
///
/// `late.epoch` is the teardown epoch the dispatch that failed began under
/// ([`late_take_retry_epoch`]). A teardown since then means that dispatch
/// was the forgotten identity's — not a retry task the teardown could stop,
/// but a first attempt already running — so nothing is scheduled for it:
/// no attempt, no record put back ([`LateTakeRetries::schedule`]).
fn retry_late_take(late: LateTake, why: &str) {
    let LateTake {
        pending,
        original,
        order_id,
        trade_pubkey_hex,
        trade_index,
        event_id,
        epoch,
        ..
    } = late;
    let registry = late_take_retries();
    let put_back = || {
        if let Ok(mut map) = pending_requests().lock() {
            map.entry(trade_pubkey_hex.clone()).or_insert(pending);
        }
    };
    let attempt = |attempt: usize| -> RedispatchFuture<'static> {
        let delay = LATE_TAKE_RETRY_DELAYS[attempt];
        crate::api::logging::blog_warn(
            "orders",
            format!(
                "late take order={} not reconciled: {why} — retrying in {}ms",
                crate::api::logging::short_id(&order_id),
                delay.as_millis(),
            ),
        );
        let scheduled_attempts = attempt + 1;
        let (order_id, event_id, trade_pubkey_hex) =
            (order_id.clone(), event_id.clone(), trade_pubkey_hex.clone());
        Box::pin(async move {
            crate::rt::time::sleep(delay).await;
            let scheduled = lock_retries(registry).attempts.contains_key(&event_id);
            if !scheduled {
                return;
            }
            // Boxed: the dispatcher's own future holds this task's, so the
            // type is erased to break the cycle.
            let redispatch: RedispatchFuture<'_> = Box::pin(dispatch_mostro_message(
                original,
                &event_id,
                &trade_pubkey_hex,
                trade_index,
            ));
            redispatch.await;
            // The attempt ran without reaching the reconcile — the node is no
            // longer the active one, or the take record is gone — so it
            // neither settled the take nor scheduled another attempt. End it
            // here rather than leave an entry nothing will ever run again.
            let stranded = {
                let mut retries = lock_retries(registry);
                let stranded = retries.attempts.get(&event_id) == Some(&scheduled_attempts);
                if stranded {
                    retries.attempts.remove(&event_id);
                }
                stranded
            };
            if stranded {
                crate::api::logging::blog_warn(
                    "orders",
                    format!(
                        "late take order={}: retry did not reach the reconcile — giving up",
                        crate::api::logging::short_id(&order_id),
                    ),
                );
            }
        })
    };
    match start_retry_task(registry, epoch, &event_id, put_back, attempt) {
        Scheduled::Attempt { .. } => {}
        Scheduled::IdentityGone => {
            crate::api::logging::blog_warn(
                "orders",
                format!(
                    "late take order={} not reconciled: {why} — its identity is gone, not retrying",
                    crate::api::logging::short_id(&order_id),
                ),
            );
        }
        Scheduled::Exhausted { attempts } => {
            crate::api::logging::blog_warn(
                "orders",
                format!(
                    "late take order={} not reconciled: {why} — giving up after {attempts} attempts",
                    crate::api::logging::short_id(&order_id),
                ),
            );
        }
    }
}

/// A future a retry runs, type-erased: the dispatch [`retry_late_take`]
/// runs again, and the attempt holding it. `Send` where tasks can move
/// across threads; on `wasm32` the storage futures are not, and nothing
/// needs them to be.
#[cfg(not(target_arch = "wasm32"))]
type RedispatchFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
type RedispatchFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>>;

/// The late-take reconcile's save, through [`persist_trade_row`], with a
/// test seam for the storage failures it must survive.
async fn save_late_take_row(db: &impl Storage, trade: &crate::api::types::TradeInfo) -> Result<()> {
    if late_take_fault("save") {
        return Err(anyhow::anyhow!("injected save failure"));
    }
    persist_trade_row(db, trade).await
}

/// The late-take reconcile's replacement of an earlier take's rows: the
/// same steps as [`persist_trade_row`], with the delete and the save in one
/// transaction ([`TradeWrite::ReplaceForOrder`]). With a test seam for the
/// failures it must survive.
async fn replace_with_late_take_row(
    db: &impl Storage,
    trade: &crate::api::types::TradeInfo,
) -> Result<()> {
    if late_take_fault("replace") {
        return Err(anyhow::anyhow!("injected replace failure"));
    }
    persist_trade_row_in(db, trade, order_book(), TradeWrite::ReplaceForOrder).await
}

#[cfg(test)]
thread_local! {
    /// Storage operations the late-take reconcile on this thread fails, once
    /// each, in any order. Thread-local: a `#[tokio::test]` runs its
    /// dispatch — retries included, on its current-thread runtime — on its
    /// own thread, so parallel tests cannot trip each other's faults.
    static LATE_TAKE_FAULTS: std::cell::RefCell<Vec<&'static str>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// An armed pause: the point it holds a late take at, the gate the take
/// waits on there, and the flag it raises once it waits.
#[cfg(test)]
type LateTakePause = (
    &'static str,
    std::sync::Arc<tokio::sync::Notify>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
);

#[cfg(test)]
thread_local! {
    /// The armed pause, if any, for the next late take on this thread to
    /// reach its point.
    static LATE_TAKE_PAUSE: std::cell::RefCell<Option<LateTakePause>> =
        const { std::cell::RefCell::new(None) };
}

/// The test seam that holds a late take at a named exposed point, so a test
/// can run the identity teardown against it: `"before-write"` (the take
/// record consumed, nothing written yet), `"after-write"` (its row written,
/// nothing bound, announced or wired yet) and `"settled"` (the take landed
/// and settled, its action arm still to write). Test builds only: each call
/// is a `#[cfg(test)]` statement, so the dispatch path carries none of it.
#[cfg(test)]
pub(super) async fn late_take_test_pause(point: &'static str) {
    let armed = LATE_TAKE_PAUSE.with(|pause| {
        let mut pause = pause.borrow_mut();
        match pause.as_ref() {
            Some((at, _, _)) if *at == point => pause.take(),
            _ => None,
        }
    });
    if let Some((_, gate, reached)) = armed {
        reached.store(true, std::sync::atomic::Ordering::SeqCst);
        gate.notified().await;
    }
}

/// Whether the test seam fails `op` now (consuming the fault). Never in a
/// non-test build.
fn late_take_fault(op: &str) -> bool {
    #[cfg(test)]
    {
        LATE_TAKE_FAULTS.with(|faults| {
            let mut faults = faults.borrow_mut();
            match faults.iter().position(|f| *f == op) {
                Some(i) => {
                    faults.remove(i);
                    true
                }
                None => false,
            }
        })
    }
    #[cfg(not(test))]
    {
        let _ = op;
        false
    }
}

/// The row of a take whose reply outlived its waiter (#566), built the way
/// the live waiter would have built it: the consumed take record carries
/// the book order the caller took, its role and a range order's chosen
/// amount, and [`classify_take_reply`] reads the reply exactly as the
/// waiter would have — status, sats, hold invoice, trade pubkeys — with or
/// without an order payload. So a late take gets the live path's row, not
/// the generic one [`rebuild_trade_from_dm`] rebuilds a replay into: a
/// fresh id, the taker's first step, the range amount it was taken at, and
/// `started_at` / `timeout_at` anchored to the reply — a row dated by its
/// order's creation instead would read as stale to the sweep's age gate
/// the moment it landed, and be wiped while the book still said `pending`.
/// It is also the only row a payload-less reply can get: a take-sell that
/// carried a default lightning address is answered `waiting-seller-to-pay`
/// with no payload (mostrod skips add-invoice — util.rs,
/// `show_hold_invoice`).
///
/// `None` hands the message back to [`dm_trade_row`]. A bond request never
/// lands here on purpose: a late `pay-bond-invoice`'s recovery belongs to
/// the daemon's idempotent re-send (docs/ANTI_ABUSE_BOND.md Phase 1).
///
/// Builds only: persisting, binding and announcing the row is
/// [`reconcile_late_take`]'s, which must hold a row before it may replace
/// anything.
fn take_snapshot_row(
    pending: &PendingRequest,
    kind: &mostro_core::message::MessageKind,
    order_id: &str,
    occurred_at: i64,
) -> Option<crate::api::types::TradeInfo> {
    let PendingRequestKind::Take {
        order,
        role,
        fiat_amount,
    } = &pending.kind
    else {
        return None;
    };
    // The nonce already correlates reply and take; a different order id on
    // the same nonce is nothing the snapshot may vouch for.
    if order.id != order_id {
        crate::api::logging::blog_warn(
            "orders",
            format!(
                "late take reply order={} does not match its record's order={} — not rebuilding",
                crate::api::logging::short_id(order_id),
                crate::api::logging::short_id(&order.id),
            ),
        );
        return None;
    }
    let DaemonReply::TakeAccepted {
        status,
        amount_sats,
        hold_invoice,
        bond: None,
        trade_pubkeys,
        ..
    } = classify_take_reply(&kind.action, &kind.payload)
    else {
        return None;
    };
    // No implied status, nothing to show — same bar the live caller's row
    // would have cleared through the reply.
    let status = status?;
    // The reply's own timestamp anchors the row, clamped so a daemon clock
    // ahead of ours cannot date a trade into the future.
    let anchor = occurred_at.min(crate::rt::unix_now());
    Some(trade_row_for_accepted_take(
        order,
        role.clone(),
        *fiat_amount,
        pending.trade_index,
        Some(status),
        amount_sats,
        hold_invoice,
        None,
        trade_pubkeys,
        anchor,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests::{
        bond_priced_small_order, daemon_message_with_nonce, drain_updates, BOND_BOLT11,
    };

    /// The book order the #566 takes snapshot into their pending record —
    /// the EUR/SEPA order of the issue's session log, from whichever side
    /// `kind` says.
    fn taken_book_order(
        order_id: &str,
        kind: crate::api::types::OrderKind,
    ) -> crate::api::types::OrderInfo {
        crate::api::types::OrderInfo {
            id: order_id.to_string(),
            kind,
            status: crate::api::types::OrderStatus::Pending,
            fiat_code: "EUR".to_string(),
            fiat_amount: Some(50.0),
            fiat_amount_min: None,
            fiat_amount_max: None,
            payment_method: "SEPA".to_string(),
            premium: 0.0,
            is_mine: false,
            created_at: 1_000,
            expires_at: None,
            amount_sats: None,
            node_pubkey: String::new(),
            rating: 0.0,
            total_reviews: 0,
            days_active: 0,
            maker_since: None,
            cashu_mint_url: None,
        }
    }

    /// The daemon's AddInvoice reply to a take, echoing its nonce, with the
    /// trade pubkeys nulled as mostrod sends it (flow.rs) — the exact message
    /// from the #566 session log.
    fn late_take_reply(
        order_uuid: uuid::Uuid,
        request_id: u64,
        created_at: u64,
    ) -> mostro_core::transport::UnwrappedMessage {
        use mostro_core::message::{Action, Payload};
        let so = mostro_core::order::SmallOrder::new(
            Some(order_uuid),
            Some(mostro_core::order::Kind::Sell),
            Some(mostro_core::order::Status::WaitingBuyerInvoice),
            11_514,
            "EUR".to_string(),
            None,
            None,
            50,
            "SEPA".to_string(),
            0,
            None,
            None,
            None,
            None,
            None,
        );
        daemon_message_with_nonce(
            order_uuid,
            Action::AddInvoice,
            Some(Payload::Order(so)),
            created_at,
            Some(request_id),
        )
    }

    /// #566: a genuine take reply arriving after the caller's 10 s timeout
    /// must land the trade while the app runs. The dispatcher consumes the
    /// detached record and hands the reply to the late-take reconcile, which
    /// builds the row from the record's snapshot of the order taken
    /// ([`take_snapshot_row`]) — the row the live waiter would have built —
    /// then persists it, binds its key and wires it up as an accepted take:
    /// no restart required. Restoring the old early return fails this test
    /// (the issue's mutation guard), and the trailing replay asserts the
    /// record was consumed exactly once: the same message again changes
    /// nothing.
    #[tokio::test]
    async fn a_late_take_reply_lands_the_trade_without_a_restart() {
        // Current timestamps: a row this recent stays inside the stale
        // sweep's age gate, so a sweep another test runs meanwhile leaves it
        // alone.
        let now = crate::rt::unix_now();
        let path =
            std::env::temp_dir().join(format!("mostro_late_take_{}.db", std::process::id()));
        let _ = crate::db::app_db::init_db(path.to_str().unwrap()).await;
        let db = crate::db::app_db::db().expect("store initialised");

        let order_uuid = uuid::Uuid::new_v4();
        let order_id = order_uuid.to_string();
        let trade_pk = "ff00ff41";
        let request_id = 4_867_897_822u64;

        // The take's record exactly as `take_order` registers it, then the
        // timeout's own sequence: the receiver drops, the waiter detaches,
        // the record survives (pending.rs).
        let (conf_tx, conf_rx) = tokio::sync::oneshot::channel::<Wake>();
        pending_requests().lock().unwrap().insert(
            trade_pk.to_string(),
            PendingRequest {
                request_id,
                trade_index: 98,
                kind: PendingRequestKind::Take {
                    order: Box::new(taken_book_order(
                        &order_id,
                        crate::api::types::OrderKind::Sell,
                    )),
                    role: TradeRole::Buyer,
                    fiat_amount: None,
                },
                tx: Some(conf_tx),
            },
        );
        drop(conf_rx);
        detach_request_waiter(trade_pk, request_id);

        let mut rx = trade_updates_tx().subscribe();
        dispatch_mostro_message(
            late_take_reply(order_uuid, request_id, now as u64),
            "test-late-take-reply",
            trade_pk,
            98,
        )
        .await;

        assert!(
            !pending_requests().lock().unwrap().contains_key(trade_pk),
            "the late reply consumes the record"
        );
        let row = db
            .get_trade_by_order_id(&order_id)
            .await
            .expect("lookup")
            .expect("the late reply persists the trade without a restart");
        assert_eq!(row.role, TradeRole::Buyer, "add-invoice addresses the buyer");
        assert!(!row.order.is_mine, "buyer of a sell order is its taker");
        assert_eq!(
            row.order.status,
            crate::api::types::OrderStatus::WaitingBuyerInvoice
        );
        assert_eq!(row.order.amount_sats, Some(11_514));
        assert_eq!(row.trade_key_index, 98, "the durable key binding is kept");
        assert_live_path_row(&row, &order_id, now);
        assert_wired_as_accepted_take(&order_id, 98, TradeRole::Buyer).await;
        assert_eq!(
            drain_updates(&mut rx, &order_id),
            vec![crate::api::types::OrderStatus::WaitingBuyerInvoice],
            "the UI is pushed as part of landing the trade"
        );

        // Replayed (another relay's copy): no record left to match, a row
        // already holding the status — no write, no update.
        dispatch_mostro_message(
            late_take_reply(order_uuid, request_id, now as u64),
            "test-late-take-replay",
            trade_pk,
            98,
        )
        .await;
        assert!(drain_updates(&mut rx, &order_id).is_empty());
        let row = db
            .get_trade_by_order_id(&order_id)
            .await
            .expect("lookup")
            .expect("row still there");
        assert_eq!(
            row.order.status,
            crate::api::types::OrderStatus::WaitingBuyerInvoice
        );
    }

    /// The row shape only the live path's builder (`trade_row_for_accepted_take`)
    /// gives a take, which the generic #394 rebuild does not (#566 review
    /// round 3): a fresh id rather than the order id, and the escrow window
    /// anchored to the reply. A row dated by its order's creation with no
    /// `timeout_at` reads as stale to the sweep's age gate at once.
    fn assert_live_path_row(
        row: &crate::api::types::TradeInfo,
        order_id: &str,
        reply_ts: i64,
    ) {
        assert_ne!(row.id, order_id, "a fresh trade id, as the live path mints");
        assert_eq!(row.started_at, reply_ts, "dated by the reply, not by the order");
        assert_eq!(
            row.timeout_at,
            Some(reply_ts + 900),
            "the escrow window the live path opens"
        );
    }

    /// #566 review round 3: a late take of a RANGE order keeps the amount it
    /// was taken at. The reply's payload names the range (min/max), which the
    /// generic rebuild reads as "no fixed amount" and drops; the take record
    /// knows the amount the user chose.
    #[tokio::test]
    async fn a_late_range_take_reply_keeps_the_amount_it_was_taken_at() {
        let path =
            std::env::temp_dir().join(format!("mostro_late_range_{}.db", std::process::id()));
        let _ = crate::db::app_db::init_db(path.to_str().unwrap()).await;
        let db = crate::db::app_db::db().expect("store initialised");

        let order_uuid = uuid::Uuid::new_v4();
        let order_id = order_uuid.to_string();
        let trade_pk = "ff00ff49";
        let request_id = 6_200_000_001u64;
        let now = crate::rt::unix_now();

        let mut book_order = taken_book_order(&order_id, crate::api::types::OrderKind::Sell);
        book_order.fiat_amount = None;
        book_order.fiat_amount_min = Some(10.0);
        book_order.fiat_amount_max = Some(100.0);
        let (conf_tx, conf_rx) = tokio::sync::oneshot::channel::<Wake>();
        pending_requests().lock().unwrap().insert(
            trade_pk.to_string(),
            PendingRequest {
                request_id,
                trade_index: 99,
                kind: PendingRequestKind::Take {
                    order: Box::new(book_order),
                    role: TradeRole::Buyer,
                    fiat_amount: Some(30.0),
                },
                tx: Some(conf_tx),
            },
        );
        drop(conf_rx);
        detach_request_waiter(trade_pk, request_id);

        use mostro_core::message::{Action, Payload};
        let so = mostro_core::order::SmallOrder::new(
            Some(order_uuid),
            Some(mostro_core::order::Kind::Sell),
            Some(mostro_core::order::Status::WaitingBuyerInvoice),
            6_900,
            "EUR".to_string(),
            Some(10),
            Some(100),
            30,
            "SEPA".to_string(),
            0,
            None,
            None,
            None,
            None,
            None,
        );
        dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::AddInvoice,
                Some(Payload::Order(so)),
                now as u64,
                Some(request_id),
            ),
            "test-late-range-take",
            trade_pk,
            99,
        )
        .await;

        let row = db
            .get_trade_by_order_id(&order_id)
            .await
            .expect("lookup")
            .expect("the late reply persists the trade");
        assert_eq!(row.order.fiat_amount, Some(30.0), "the amount it was taken at");
        assert_eq!(row.order.fiat_amount_min, Some(10.0));
        assert_eq!(row.order.fiat_amount_max, Some(100.0));
        assert_eq!(row.order.amount_sats, Some(6_900));
        assert_live_path_row(&row, &order_id, now);
    }

    /// What the take record declines still lands: a reply naming a different
    /// order under the take's nonce is nothing the snapshot vouches for, so
    /// the generic #394 rebuild takes it from the message, as it would on
    /// the next start's replay (#566 review round 3).
    #[tokio::test]
    async fn a_late_take_reply_the_record_declines_falls_back_to_the_message() {
        let path =
            std::env::temp_dir().join(format!("mostro_late_fallback_{}.db", std::process::id()));
        let _ = crate::db::app_db::init_db(path.to_str().unwrap()).await;
        let db = crate::db::app_db::db().expect("store initialised");

        let order_uuid = uuid::Uuid::new_v4();
        let order_id = order_uuid.to_string();
        let trade_pk = "ff00ff4a";
        let request_id = 6_200_000_002u64;
        let now = crate::rt::unix_now();

        let (conf_tx, conf_rx) = tokio::sync::oneshot::channel::<Wake>();
        pending_requests().lock().unwrap().insert(
            trade_pk.to_string(),
            PendingRequest {
                request_id,
                trade_index: 95,
                kind: PendingRequestKind::Take {
                    order: Box::new(taken_book_order(
                        "some-other-order",
                        crate::api::types::OrderKind::Sell,
                    )),
                    role: TradeRole::Buyer,
                    fiat_amount: None,
                },
                tx: Some(conf_tx),
            },
        );
        drop(conf_rx);
        detach_request_waiter(trade_pk, request_id);

        dispatch_mostro_message(
            late_take_reply(order_uuid, request_id, now as u64),
            "test-late-take-fallback",
            trade_pk,
            95,
        )
        .await;

        let row = db
            .get_trade_by_order_id(&order_id)
            .await
            .expect("lookup")
            .expect("the message itself still rebuilds the row");
        assert_eq!(row.role, TradeRole::Buyer, "add-invoice addresses the buyer");
        assert_eq!(row.id, order_id, "the generic #394 row, not the take's");
        assert_eq!(row.trade_key_index, 95);
        assert_eq!(
            row.order.node_pubkey,
            active_mostro_pubkey(),
            "the row rebuilt from the message belongs to the node that sent it",
        );
    }

    /// What the live path leaves behind an accepted take besides its row
    /// (`wire_accepted_take`): the session the chat API reads keys from, on
    /// this take's key. A late take must leave the same (#566 review round
    /// 2). The single-order watch is not asserted: without a relay pool its
    /// task releases itself as soon as it runs.
    async fn assert_wired_as_accepted_take(order_id: &str, trade_index: u32, role: TradeRole) {
        let session = crate::mostro::session::session_manager()
            .get_session(order_id)
            .await
            .expect("the late take installs its session");
        assert_eq!(session.trade_key_index, trade_index, "session on this take's key");
        assert_eq!(session.role, role);
    }

    /// #566 review round 2: a row an EARLIER take of the order left behind —
    /// its `Canceled` never reached us, the order went back to the book and
    /// was taken again — must not absorb the late reply of the new take. The
    /// new take's record proves its generation, so its row replaces the old
    /// one, as `persist_confirmed_take` does on the live path, and the old
    /// take's session gives way to this one's.
    async fn late_retake_supersedes_older_row(
        old_status: crate::api::types::OrderStatus,
        trade_pk: &str,
        request_id: u64,
    ) {
        let path = std::env::temp_dir().join(format!(
            "mostro_late_retake_{}_{}.db",
            trade_pk,
            std::process::id()
        ));
        let _ = crate::db::app_db::init_db(path.to_str().unwrap()).await;
        let db = crate::db::app_db::db().expect("store initialised");

        let order_uuid = uuid::Uuid::new_v4();
        let order_id = order_uuid.to_string();
        let book_order = taken_book_order(&order_id, crate::api::types::OrderKind::Sell);
        // Current timestamps, unlike the other #566 tests: rows this recent
        // stay inside the stale sweep's age gate, so a sweep another test
        // runs meanwhile leaves them alone.
        let now = crate::rt::unix_now();

        // Take N (97), as its seller — a different role on purpose, so a row
        // or session that survived would show.
        let old = trade_row_for_accepted_take(
            &book_order,
            TradeRole::Seller,
            None,
            97,
            Some(old_status),
            None,
            None,
            None,
            crate::mostro::pending::TradePubkeys::default(),
            now - 60,
        );
        persist_trade_row(db, &old).await.expect("seed the older take's row");
        crate::mostro::session::session_manager()
            .install_session(order_id.clone(), TradeRole::Seller, 97, book_order.clone())
            .await
            .expect("seed the older take's session");

        // Take N+1 (98), as its buyer, whose reply outlives the waiter.
        let (conf_tx, conf_rx) = tokio::sync::oneshot::channel::<Wake>();
        pending_requests().lock().unwrap().insert(
            trade_pk.to_string(),
            PendingRequest {
                request_id,
                trade_index: 98,
                kind: PendingRequestKind::Take {
                    order: Box::new(book_order),
                    role: TradeRole::Buyer,
                    fiat_amount: None,
                },
                tx: Some(conf_tx),
            },
        );
        drop(conf_rx);
        detach_request_waiter(trade_pk, request_id);

        let mut rx = trade_updates_tx().subscribe();
        dispatch_mostro_message(
            late_take_reply(order_uuid, request_id, now as u64),
            "test-late-retake",
            trade_pk,
            98,
        )
        .await;

        let row = db
            .get_trade_by_order_id(&order_id)
            .await
            .expect("lookup")
            .expect("the accepted retake lands its row");
        assert_ne!(row.id, old.id, "the older take's row is replaced, not mutated");
        assert_eq!(row.role, TradeRole::Buyer, "the retake's role, not the older take's");
        assert_eq!(row.trade_key_index, 98, "the retake's key, not the older take's");
        assert_eq!(
            row.order.status,
            crate::api::types::OrderStatus::WaitingBuyerInvoice
        );
        assert_eq!(
            drain_updates(&mut rx, &order_id),
            vec![crate::api::types::OrderStatus::WaitingBuyerInvoice],
        );
        assert_wired_as_accepted_take(&order_id, 98, TradeRole::Buyer).await;
    }

    #[tokio::test]
    async fn a_late_retake_replaces_an_older_takes_live_row() {
        late_retake_supersedes_older_row(
            crate::api::types::OrderStatus::WaitingPayment,
            "ff00ff47",
            6_100_000_001,
        )
        .await;
    }

    /// The terminal flavour: the older take's row reads `Canceled`, which the
    /// terminal guard would otherwise read as this order's status and drop
    /// the retake's reply whole — the #566 symptom again.
    #[tokio::test]
    async fn a_late_retake_replaces_an_older_takes_canceled_row() {
        late_retake_supersedes_older_row(
            crate::api::types::OrderStatus::Canceled,
            "ff00ff48",
            6_100_000_002,
        )
        .await;
    }

    // ── #566 review round 4: replace only what can be replaced, retry ────
    // what the store failed ────────────────────────────────────────────────

    /// A fresh store and order for one round-4 scenario, the take's book
    /// order (a sell order, taken as its buyer) and the current time.
    async fn round4_setup(
        tag: &str,
    ) -> (
        &'static impl crate::db::Storage,
        uuid::Uuid,
        String,
        crate::api::types::OrderInfo,
        i64,
    ) {
        let path = std::env::temp_dir().join(format!(
            "mostro_late_take_r4_{tag}_{}.db",
            std::process::id()
        ));
        let _ = crate::db::app_db::init_db(path.to_str().unwrap()).await;
        let db = crate::db::app_db::db().expect("store initialised");
        let order_uuid = uuid::Uuid::new_v4();
        let order_id = order_uuid.to_string();
        let book_order = taken_book_order(&order_id, crate::api::types::OrderKind::Sell);
        (db, order_uuid, order_id, book_order, crate::rt::unix_now())
    }

    /// The row and session an earlier take of the order (index 97, as its
    /// seller) left behind, still open.
    async fn seed_earlier_take(
        db: &impl crate::db::Storage,
        book_order: &crate::api::types::OrderInfo,
        now: i64,
    ) -> crate::api::types::TradeInfo {
        let old = trade_row_for_accepted_take(
            book_order,
            TradeRole::Seller,
            None,
            97,
            Some(crate::api::types::OrderStatus::WaitingPayment),
            None,
            None,
            None,
            crate::mostro::pending::TradePubkeys::default(),
            now - 60,
        );
        persist_trade_row(db, &old).await.expect("seed the earlier take's row");
        crate::mostro::session::session_manager()
            .install_session(old.order.id.clone(), TradeRole::Seller, 97, book_order.clone())
            .await
            .expect("seed the earlier take's session");
        old
    }

    /// Take 98 of `book_order`, as its buyer, whose waiter already timed out.
    fn register_timed_out_take(
        trade_pk: &str,
        request_id: u64,
        book_order: &crate::api::types::OrderInfo,
    ) {
        let (conf_tx, conf_rx) = tokio::sync::oneshot::channel::<Wake>();
        pending_requests().lock().unwrap().insert(
            trade_pk.to_string(),
            PendingRequest {
                request_id,
                trade_index: 98,
                kind: PendingRequestKind::Take {
                    order: Box::new(book_order.clone()),
                    role: TradeRole::Buyer,
                    fiat_amount: None,
                },
                tx: Some(conf_tx),
            },
        );
        drop(conf_rx);
        detach_request_waiter(trade_pk, request_id);
    }

    fn inject_late_take_fault(op: &'static str) {
        LATE_TAKE_FAULTS.with(|faults| faults.borrow_mut().push(op));
    }

    /// The earlier take's row, untouched: same id, role and key.
    async fn assert_earlier_row_kept(
        db: &impl crate::db::Storage,
        old: &crate::api::types::TradeInfo,
    ) {
        let row = db
            .get_trade_by_order_id(&old.order.id)
            .await
            .expect("lookup")
            .expect("the earlier take's row is kept");
        assert_eq!(row.id, old.id, "the earlier row itself, not a replacement");
        assert_eq!(row.role, TradeRole::Seller);
        assert_eq!(row.trade_key_index, 97);
        assert_eq!(row.order.status, old.order.status, "and nothing applied to it");
    }

    /// A late `pay-bond-invoice` for take 98 over take 97's row: neither
    /// builder rebuilds a bond request, so there is nothing to replace the
    /// earlier row with — it stays, and the reply does not reach the arms,
    /// the only row there being another trade's.
    #[tokio::test]
    async fn a_late_bond_request_leaves_an_earlier_takes_row_in_place() {
        use mostro_core::message::{Action, Payload};
        let (db, order_uuid, order_id, book_order, now) = round4_setup("bond").await;
        let old = seed_earlier_take(db, &book_order, now).await;
        let trade_pk = "ff00ff4b";
        register_timed_out_take(trade_pk, 6_300_000_001, &book_order);

        let mut rx = trade_updates_tx().subscribe();
        dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::PayBondInvoice,
                Some(Payload::PaymentRequest(
                    Some(bond_priced_small_order(order_uuid, 1_200)),
                    BOND_BOLT11.to_string(),
                    Some(1_200),
                )),
                now as u64,
                Some(6_300_000_001),
            ),
            "test-r4-bond",
            trade_pk,
            98,
        )
        .await;

        assert_earlier_row_kept(db, &old).await;
        assert!(drain_updates(&mut rx, &order_id).is_empty());
        assert!(
            !pending_requests().lock().unwrap().contains_key(trade_pk),
            "a declined reply is a verdict, not a failure: nothing to retry"
        );
    }

    /// A late take reply the status cursor refuses — the order already
    /// applied a newer message — over an earlier take's row: not admitted,
    /// so nothing is replaced.
    #[tokio::test]
    async fn a_late_take_reply_the_cursor_refuses_leaves_an_earlier_takes_row_in_place() {
        let (db, order_uuid, order_id, book_order, now) = round4_setup("cursor").await;
        let old = seed_earlier_take(db, &book_order, now).await;
        db.set_setting(
            &crate::db::settings_keys::status_cursor(&order_id),
            &(now + 30).to_string(),
        )
        .await
        .expect("advance the cursor past the reply");
        let trade_pk = "ff00ff4c";
        register_timed_out_take(trade_pk, 6_300_000_002, &book_order);

        let mut rx = trade_updates_tx().subscribe();
        dispatch_mostro_message(
            late_take_reply(order_uuid, 6_300_000_002, now as u64),
            "test-r4-cursor",
            trade_pk,
            98,
        )
        .await;

        assert_earlier_row_kept(db, &old).await;
        assert!(drain_updates(&mut rx, &order_id).is_empty());
    }

    /// Polls until `order_id`'s row is the one of `trade_index` AND its
    /// landing was announced, giving a scheduled late-take retry (5 ms apart
    /// under test) time to run on this test's runtime: the reconcile saves
    /// the row, then awaits the key binding before it announces, so a row
    /// can be visible a moment before its update. Returns the row and the
    /// updates announced for the order meanwhile — drained on every poll, so
    /// the shared channel cannot overrun `rx` while the retry is pending.
    async fn wait_for_take_row(
        db: &impl crate::db::Storage,
        order_id: &str,
        trade_index: u32,
        rx: &mut broadcast::Receiver<crate::api::types::TradeUpdate>,
    ) -> (
        Option<crate::api::types::TradeInfo>,
        Vec<crate::api::types::OrderStatus>,
    ) {
        let mut announced = Vec::new();
        for _ in 0..400 {
            announced.extend(drain_updates(rx, order_id));
            if let Some(row) = db.get_trade_by_order_id(order_id).await.expect("lookup") {
                if row.trade_key_index == trade_index && !announced.is_empty() {
                    announced.extend(drain_updates(rx, order_id));
                    return (Some(row), announced);
                }
            }
            crate::rt::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        (None, announced)
    }

    /// Why a scheduled retry did not land, for a failing assertion: the
    /// global state another test running alongside may have moved.
    fn late_take_retry_state(trade_pk: &str, event_id: &str, sender_hex: &str) -> String {
        format!(
            "record_present={} retry_scheduled={} active_node_is_sender={}",
            pending_requests().lock().unwrap().contains_key(trade_pk),
            with_late_take_retries(|r| r.attempts.contains_key(event_id)),
            active_mostro_pubkey() == sender_hex,
        )
    }

    /// Lets every scheduled retry that is going to run, run.
    async fn let_late_take_retries_run() {
        crate::rt::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    /// #566 review round 5: the replacement fails twice in a row. Each time
    /// it is one transaction, so the order keeps the earlier take's row and
    /// nothing is announced; the reply is retried locally — no redelivery
    /// needed, none would come — and the third attempt lands the take.
    #[tokio::test]
    async fn two_failed_replacements_keep_the_earlier_row_and_a_local_retry_lands_the_take() {
        let (db, order_uuid, order_id, book_order, now) = round4_setup("replace").await;
        let old = seed_earlier_take(db, &book_order, now).await;
        let trade_pk = "ff00ff4d";
        let request_id = 6_300_000_003u64;
        register_timed_out_take(trade_pk, request_id, &book_order);

        inject_late_take_fault("replace");
        inject_late_take_fault("replace");
        let sender_hex = active_mostro_pubkey();
        let mut rx = trade_updates_tx().subscribe();
        dispatch_mostro_message(
            late_take_reply(order_uuid, request_id, now as u64),
            "test-r5-replace",
            trade_pk,
            98,
        )
        .await;

        // The first attempt failed: nothing changed, nothing announced.
        assert_earlier_row_kept(db, &old).await;
        assert!(drain_updates(&mut rx, &order_id).is_empty());

        let (row, announced) = wait_for_take_row(db, &order_id, 98, &mut rx).await;
        let row = row.unwrap_or_else(|| {
            panic!(
                "a local retry lands the take, without a redelivery — {}",
                late_take_retry_state(trade_pk, "test-r5-replace", &sender_hex)
            )
        });
        assert_ne!(row.id, old.id);
        assert_eq!(row.role, TradeRole::Buyer);
        assert_eq!(
            announced,
            vec![crate::api::types::OrderStatus::WaitingBuyerInvoice],
            "announced once, when it landed"
        );
        assert!(!pending_requests().lock().unwrap().contains_key(trade_pk));
        assert!(!with_late_take_retries(|r| r.attempts.contains_key("test-r5-replace")));
        assert_wired_as_accepted_take(&order_id, 98, TradeRole::Buyer).await;
    }

    /// ermeme's durability case: a payload-less `waiting-seller-to-pay`,
    /// whose only source of a row is the take record, and a first write
    /// that fails. The record is kept with the reply and the retry rebuilds
    /// the row from it — without a restart, and without the event coming
    /// back from a relay (nostr-sdk would not notify it again).
    #[tokio::test]
    async fn a_failed_first_write_of_a_payload_less_late_take_is_recovered_by_a_local_retry() {
        use mostro_core::message::Action;
        let (db, order_uuid, order_id, book_order, now) = round4_setup("wstp").await;
        let trade_pk = "ff00ff4f";
        let request_id = 6_300_000_005u64;
        register_timed_out_take(trade_pk, request_id, &book_order);

        inject_late_take_fault("save");
        let sender_hex = active_mostro_pubkey();
        let mut rx = trade_updates_tx().subscribe();
        dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::WaitingSellerToPay,
                None,
                now as u64,
                Some(request_id),
            ),
            "test-r5-wstp",
            trade_pk,
            98,
        )
        .await;

        assert!(
            db.get_trade_by_order_id(&order_id).await.expect("lookup").is_none(),
            "the failed write left no row"
        );
        assert!(drain_updates(&mut rx, &order_id).is_empty(), "and announced nothing");

        let (row, announced) = wait_for_take_row(db, &order_id, 98, &mut rx).await;
        let row = row.unwrap_or_else(|| {
            panic!(
                "the retry rebuilds the row from the take record — {}",
                late_take_retry_state(trade_pk, "test-r5-wstp", &sender_hex)
            )
        });
        assert_eq!(row.role, TradeRole::Buyer);
        assert_eq!(row.order.status, crate::api::types::OrderStatus::WaitingPayment);
        assert_eq!(announced, vec![crate::api::types::OrderStatus::WaitingPayment]);
        assert!(
            !pending_requests().lock().unwrap().contains_key(trade_pk),
            "consumed exactly once, by the attempt that landed"
        );
        assert_wired_as_accepted_take(&order_id, 98, TradeRole::Buyer).await;
    }

    /// A store that keeps failing: every scheduled attempt fails, then the
    /// take is given up — bounded, nothing left scheduled or registered,
    /// and the earlier row still the order's.
    #[tokio::test]
    async fn a_late_take_the_store_keeps_failing_is_given_up_with_the_earlier_row_kept() {
        let (db, order_uuid, order_id, book_order, now) = round4_setup("giveup").await;
        let old = seed_earlier_take(db, &book_order, now).await;
        let trade_pk = "ff00ff4e";
        let request_id = 6_300_000_004u64;
        register_timed_out_take(trade_pk, request_id, &book_order);

        for _ in 0..=LATE_TAKE_RETRY_DELAYS.len() {
            inject_late_take_fault("replace");
        }
        let mut rx = trade_updates_tx().subscribe();
        dispatch_mostro_message(
            late_take_reply(order_uuid, request_id, now as u64),
            "test-r5-giveup",
            trade_pk,
            98,
        )
        .await;
        let_late_take_retries_run().await;

        assert_earlier_row_kept(db, &old).await;
        assert!(drain_updates(&mut rx, &order_id).is_empty());
        assert!(!with_late_take_retries(|r| r.attempts.contains_key("test-r5-giveup")));
        assert!(
            !pending_requests().lock().unwrap().contains_key(trade_pk),
            "a given-up take leaves no record behind"
        );
        LATE_TAKE_FAULTS.with(|faults| {
            assert!(faults.borrow().is_empty(), "every attempt ran: 1 + one per delay")
        });
    }

    /// A retry scheduled for an identity that is then forgotten never runs:
    /// it would write the old identity's trade into the new one's store. The
    /// source guard below pins that the identity release calls
    /// `forget_late_take_retries`.
    #[tokio::test]
    async fn a_forgotten_identity_cancels_its_scheduled_late_take_retries() {
        use mostro_core::message::Action;
        let (db, order_uuid, order_id, book_order, now) = round4_setup("forget").await;
        let trade_pk = "ff00ff5a";
        let request_id = 6_300_000_001u64;
        register_timed_out_take(trade_pk, request_id, &book_order);

        inject_late_take_fault("save");
        dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::WaitingSellerToPay,
                None,
                now as u64,
                Some(request_id),
            ),
            "test-r5-forget",
            trade_pk,
            98,
        )
        .await;
        forget_late_take_retries().await;
        let_late_take_retries_run().await;

        assert!(db.get_trade_by_order_id(&order_id).await.expect("lookup").is_none());
        pending_requests().lock().unwrap().remove(trade_pk);
    }

    /// A retry that runs but never reaches the reconcile — here the take
    /// record is gone by then — settles nothing and schedules nothing: it
    /// is ended instead of left in the registry for good.
    #[tokio::test]
    async fn a_retry_that_cannot_reach_the_reconcile_is_ended_not_stranded() {
        use mostro_core::message::Action;
        let (db, order_uuid, order_id, book_order, now) = round4_setup("strand").await;
        let trade_pk = "ff00ff5b";
        let request_id = 6_300_000_006u64;
        register_timed_out_take(trade_pk, request_id, &book_order);

        inject_late_take_fault("save");
        dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::WaitingSellerToPay,
                None,
                now as u64,
                Some(request_id),
            ),
            "test-r5-strand",
            trade_pk,
            98,
        )
        .await;
        assert!(with_late_take_retries(|r| r.attempts.contains_key("test-r5-strand")));
        pending_requests().lock().unwrap().remove(trade_pk);
        let_late_take_retries_run().await;

        assert!(db.get_trade_by_order_id(&order_id).await.expect("lookup").is_none());
        assert!(
            !with_late_take_retries(|r| r.attempts.contains_key("test-r5-strand")),
            "the stranded retry is ended"
        );
    }

    /// Arms the late-take pause at `point` and waits until a retry is held
    /// there; returns the gate that lets it go.
    async fn pause_late_take_at(point: &'static str) -> std::sync::Arc<tokio::sync::Notify> {
        let gate = std::sync::Arc::new(tokio::sync::Notify::new());
        let reached = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        LATE_TAKE_PAUSE
            .with(|pause| *pause.borrow_mut() = Some((point, gate.clone(), reached.clone())));
        for _ in 0..400 {
            if reached.load(std::sync::atomic::Ordering::SeqCst) {
                return gate;
            }
            crate::rt::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("no late take reached the {point} pause");
    }

    /// ermeme's round-6 ordering: a retry that already LANDED and settled its
    /// take, still inside its dispatch with the action arm to run, when the
    /// identity goes. Settling ends the retry bookkeeping, not the task: the
    /// teardown still stops it, so the arm's writes never happen after it.
    ///
    /// Watched through the order's status cursor: a settings row keyed by
    /// this test's own order, and the arm's first write — the book update
    /// comes after it, so no cursor means no book write either. The book
    /// itself is process-wide and emptied by tests running alongside, so it
    /// cannot be asserted on here.
    #[tokio::test]
    async fn deleting_the_identity_stops_a_settled_retry_before_its_action_arm_writes() {
        let (db, order_uuid, order_id, book_order, now) = round4_setup("settled").await;
        let trade_pk = "ff00ff5d";
        let request_id = 6_300_000_008u64;
        register_timed_out_take(trade_pk, request_id, &book_order);

        inject_late_take_fault("save");
        dispatch_mostro_message(
            late_take_reply(order_uuid, request_id, now as u64),
            "test-r7-settled",
            trade_pk,
            98,
        )
        .await;

        let gate = pause_late_take_at("settled").await;
        assert!(
            db.get_trade_by_order_id(&order_id).await.expect("lookup").is_some(),
            "the retry landed its take before the pause"
        );
        assert!(
            !with_late_take_retries(|r| r.attempts.contains_key("test-r7-settled")),
            "and settled it"
        );
        assert!(
            with_late_take_retries(|r| !r.tasks.is_empty()),
            "while its task is still running, and still owned"
        );

        forget_late_take_retries().await;
        gate.notify_one();
        let_late_take_retries_run().await;

        assert_eq!(
            db.get_setting(&crate::db::settings_keys::status_cursor(&order_id))
                .await
                .expect("lookup"),
            None,
            "the action arm never recorded the status cursor"
        );
    }

    /// The control for the test above: the same settled retry, let go with
    /// the identity still in place, does run its action arm and records the
    /// order's status cursor. So the test above proves that write was
    /// stopped, not that it never happens.
    #[tokio::test]
    async fn a_settled_retry_runs_its_action_arm_when_nothing_stops_it() {
        let (db, order_uuid, order_id, book_order, now) = round4_setup("settled-control").await;
        let trade_pk = "ff00ff5e";
        let request_id = 6_300_000_009u64;
        register_timed_out_take(trade_pk, request_id, &book_order);

        inject_late_take_fault("save");
        dispatch_mostro_message(
            late_take_reply(order_uuid, request_id, now as u64),
            "test-r7-settled-control",
            trade_pk,
            98,
        )
        .await;

        let gate = pause_late_take_at("settled").await;
        gate.notify_one();
        let_late_take_retries_run().await;

        assert!(
            db.get_setting(&crate::db::settings_keys::status_cursor(&order_id))
                .await
                .expect("lookup")
                .is_some(),
            "the action arm recorded the status cursor"
        );
        assert!(
            with_late_take_retries(|r| r.tasks.is_empty()),
            "a finished task leaves the registry"
        );
    }

    /// The teardown can stop retry TASKS only. A FIRST attempt already
    /// running when the identity goes is not one: held before a write that
    /// would fail, the identity is torn down, and the attempt stops at its
    /// own epoch check — the write is never tried, so no retry is scheduled
    /// for the forgotten identity: no attempt counted, no task, no take
    /// record put back, no row later. (The epoch also refuses a retry from a
    /// write the teardown overtakes mid-flight: `schedule`'s `IdentityGone`.)
    #[tokio::test]
    async fn a_first_attempt_running_through_the_teardown_schedules_no_retry() {
        use mostro_core::message::Action;
        let (db, order_uuid, order_id, book_order, now) = round4_setup("epoch").await;
        let trade_pk = "ff00ff5f";
        let request_id = 6_300_000_010u64;
        register_timed_out_take(trade_pk, request_id, &book_order);

        inject_late_take_fault("save");
        let first = tokio::spawn(dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::WaitingSellerToPay,
                None,
                now as u64,
                Some(request_id),
            ),
            "test-r8-epoch",
            trade_pk,
            98,
        ));
        let gate = pause_late_take_at("before-write").await;

        forget_late_take_retries().await;
        gate.notify_one();
        first.await.expect("the first attempt ran to its end");
        let_late_take_retries_run().await;

        LATE_TAKE_FAULTS.with(|faults| {
            let mut faults = faults.borrow_mut();
            assert_eq!(
                *faults,
                vec!["save"],
                "the first attempt stopped before its write"
            );
            faults.clear();
        });
        assert!(
            with_late_take_retries(|r| !r.attempts.contains_key("test-r8-epoch")
                && r.tasks.is_empty()),
            "no retry scheduled for the forgotten identity"
        );
        assert!(
            !pending_requests().lock().unwrap().contains_key(trade_pk),
            "and no take record put back"
        );
        assert!(db.get_trade_by_order_id(&order_id).await.expect("lookup").is_none());
    }

    /// No key binding, no session and no update for `order_id`: what a late
    /// take of a forgotten identity must not leave behind.
    async fn assert_nothing_bound_announced_or_wired(
        db: &impl crate::db::Storage,
        order_id: &str,
        rx: &mut tokio::sync::broadcast::Receiver<crate::api::types::TradeUpdate>,
    ) {
        assert_eq!(
            db.get_trade_key(order_id).await.expect("lookup"),
            None,
            "no key binding"
        );
        assert!(
            crate::mostro::session::session_manager()
                .get_session(order_id)
                .await
                .is_none(),
            "no session"
        );
        assert!(drain_updates(rx, order_id).is_empty(), "no update");
    }

    /// The teardown cannot stop a FIRST attempt — it is no retry task — so
    /// the attempt checks the epoch itself (CodeRabbit on #570). Held right
    /// before a write that would succeed while the identity goes, then let
    /// go, it writes nothing: no row, no key binding, no session, no update.
    #[tokio::test]
    async fn a_first_attempt_the_teardown_overtook_before_its_write_lands_nothing() {
        use mostro_core::message::Action;
        let (db, order_uuid, order_id, book_order, now) = round4_setup("overtook-before").await;
        let trade_pk = "ff00ffa1";
        let request_id = 6_300_000_021u64;
        register_timed_out_take(trade_pk, request_id, &book_order);

        let mut rx = trade_updates_tx().subscribe();
        let first = tokio::spawn(dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::WaitingSellerToPay,
                None,
                now as u64,
                Some(request_id),
            ),
            "test-r9-before",
            trade_pk,
            98,
        ));
        let gate = pause_late_take_at("before-write").await;

        forget_late_take_retries().await;
        gate.notify_one();
        first.await.expect("the first attempt ran to its end");

        assert!(
            db.get_trade_by_order_id(&order_id).await.expect("lookup").is_none(),
            "no row of the old identity's"
        );
        assert_nothing_bound_announced_or_wired(db, &order_id, &mut rx).await;
    }

    /// The same first attempt overtaken right AFTER its write: the row is
    /// written — the identity teardown's own wipe removes it — but nothing
    /// that follows the write may run for the forgotten identity: no key
    /// binding, no session, no update.
    #[tokio::test]
    async fn a_first_attempt_the_teardown_overtook_after_its_write_binds_and_wires_nothing() {
        use mostro_core::message::Action;
        let (db, order_uuid, order_id, book_order, now) = round4_setup("overtook-after").await;
        let trade_pk = "ff00ffa2";
        let request_id = 6_300_000_022u64;
        register_timed_out_take(trade_pk, request_id, &book_order);

        let mut rx = trade_updates_tx().subscribe();
        let first = tokio::spawn(dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::WaitingSellerToPay,
                None,
                now as u64,
                Some(request_id),
            ),
            "test-r9-after",
            trade_pk,
            98,
        ));
        let gate = pause_late_take_at("after-write").await;
        assert!(
            db.get_trade_by_order_id(&order_id).await.expect("lookup").is_some(),
            "held after its write"
        );

        forget_late_take_retries().await;
        gate.notify_one();
        first.await.expect("the first attempt ran to its end");

        assert_nothing_bound_announced_or_wired(db, &order_id, &mut rx).await;
    }

    /// The identity release stops the late-take retries FIRST, awaited: a
    /// retry still running after the in-memory clears below it would write
    /// the old identity's binding back into them.
    #[test]
    fn releasing_the_identity_stops_its_late_take_retries_first() {
        let source = include_str!("../orders.rs");
        let start = source
            .find("pub(crate) async fn release_identity_subscriptions()")
            .expect("the identity release exists");
        let end = start + source[start..].find("\n}\n").expect("the function ends");
        let body = &source[start..end];
        let stop = body
            .find("forget_late_take_retries().await")
            .expect("the retries are stopped, and waited for");
        for clear in ["single_order_tasks()", "global_dm_keys()", "trade_key_map()"] {
            let at = body.find(clear).expect("the release clears it");
            assert!(stop < at, "the retries stop before {clear} is cleared");
        }
    }

    /// ermeme's ordering: a retry already INSIDE the reconcile — its take
    /// record consumed, about to write — when the identity is deleted. The
    /// teardown stops it there and waits for it; released afterwards, it
    /// writes nothing: no row, no key binding, no session, no update.
    #[tokio::test]
    async fn deleting_the_identity_stops_a_late_take_retry_already_reconciling() {
        use mostro_core::message::Action;
        let (db, order_uuid, order_id, book_order, now) = round4_setup("inflight").await;
        let trade_pk = "ff00ff5c";
        let request_id = 6_300_000_007u64;
        register_timed_out_take(trade_pk, request_id, &book_order);

        // The first attempt fails its write, so a retry is scheduled.
        inject_late_take_fault("save");
        let mut rx = trade_updates_tx().subscribe();
        dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::WaitingSellerToPay,
                None,
                now as u64,
                Some(request_id),
            ),
            "test-r6-inflight",
            trade_pk,
            98,
        )
        .await;

        // The retry is held inside its reconcile, right before the write.
        let gate = pause_late_take_at("before-write").await;
        assert!(
            !pending_requests().lock().unwrap().contains_key(trade_pk),
            "and consumed its take record"
        );

        // The identity goes; only then is the retry let go.
        forget_late_take_retries().await;
        gate.notify_one();
        let_late_take_retries_run().await;

        assert!(
            db.get_trade_by_order_id(&order_id).await.expect("lookup").is_none(),
            "no row of the old identity's"
        );
        assert_eq!(
            db.get_trade_key(&order_id).await.expect("lookup"),
            None,
            "no key binding"
        );
        assert!(
            crate::mostro::session::session_manager()
                .get_session(&order_id)
                .await
                .is_none(),
            "no session"
        );
        assert!(drain_updates(&mut rx, &order_id).is_empty(), "no update");
    }

    /// The race inside the take's timeout (#566): `timeout()` has dropped
    /// the receiver but `detach_request_waiter` has not run yet, so the
    /// record still holds a waiter whose send fails. Same outcome as the
    /// detached case — the dispatcher takes its guard back and lands the
    /// trade through the normal dispatch.
    #[tokio::test]
    async fn a_take_reply_racing_the_timeouts_detach_still_lands_the_trade() {
        // Current timestamps: a row this recent stays inside the stale
        // sweep's age gate, so a sweep another test runs meanwhile leaves it
        // alone.
        let now = crate::rt::unix_now();
        let path =
            std::env::temp_dir().join(format!("mostro_late_take_race_{}.db", std::process::id()));
        let _ = crate::db::app_db::init_db(path.to_str().unwrap()).await;
        let db = crate::db::app_db::db().expect("store initialised");

        let order_uuid = uuid::Uuid::new_v4();
        let order_id = order_uuid.to_string();
        let trade_pk = "ff00ff42";
        let request_id = 7_311_002_954u64;

        let (conf_tx, conf_rx) = tokio::sync::oneshot::channel::<Wake>();
        pending_requests().lock().unwrap().insert(
            trade_pk.to_string(),
            PendingRequest {
                request_id,
                trade_index: 99,
                kind: PendingRequestKind::Take {
                    order: Box::new(taken_book_order(
                        &order_id,
                        crate::api::types::OrderKind::Sell,
                    )),
                    role: TradeRole::Buyer,
                    fiat_amount: None,
                },
                tx: Some(conf_tx),
            },
        );
        drop(conf_rx); // timeout fired; detach has not happened yet

        let mut rx = trade_updates_tx().subscribe();
        dispatch_mostro_message(
            late_take_reply(order_uuid, request_id, now as u64),
            "test-late-take-race",
            trade_pk,
            99,
        )
        .await;

        assert!(!pending_requests().lock().unwrap().contains_key(trade_pk));
        let row = db
            .get_trade_by_order_id(&order_id)
            .await
            .expect("lookup")
            .expect("the failed send falls through and persists the trade");
        assert_eq!(
            row.order.status,
            crate::api::types::OrderStatus::WaitingBuyerInvoice
        );
        assert_eq!(
            drain_updates(&mut rx, &order_id),
            vec![crate::api::types::OrderStatus::WaitingBuyerInvoice],
        );
    }

    /// #566, payload-less flank: a take-sell that carried a default
    /// lightning address is answered `waiting-seller-to-pay` with NO payload
    /// (mostrod skips add-invoice), so there is no order in the message to
    /// rebuild from — the row comes from the take record's own snapshot.
    /// The trailing replay asserts the record was consumed exactly once.
    #[tokio::test]
    async fn a_late_waiting_seller_to_pay_reply_lands_the_trade_from_its_take_record() {
        // Current timestamps: a row this recent stays inside the stale
        // sweep's age gate, so a sweep another test runs meanwhile leaves it
        // alone.
        let now = crate::rt::unix_now();
        use mostro_core::message::Action;
        let path =
            std::env::temp_dir().join(format!("mostro_late_wstp_{}.db", std::process::id()));
        let _ = crate::db::app_db::init_db(path.to_str().unwrap()).await;
        let db = crate::db::app_db::db().expect("store initialised");

        let order_uuid = uuid::Uuid::new_v4();
        let order_id = order_uuid.to_string();
        let trade_pk = "ff00ff44";
        let request_id = 9_990_001_111u64;

        let (conf_tx, conf_rx) = tokio::sync::oneshot::channel::<Wake>();
        pending_requests().lock().unwrap().insert(
            trade_pk.to_string(),
            PendingRequest {
                request_id,
                trade_index: 96,
                kind: PendingRequestKind::Take {
                    order: Box::new(taken_book_order(
                        &order_id,
                        crate::api::types::OrderKind::Sell,
                    )),
                    role: TradeRole::Buyer,
                    fiat_amount: None,
                },
                tx: Some(conf_tx),
            },
        );
        drop(conf_rx);
        detach_request_waiter(trade_pk, request_id);

        let mut rx = trade_updates_tx().subscribe();
        dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::WaitingSellerToPay,
                None,
                now as u64,
                Some(request_id),
            ),
            "test-late-wstp",
            trade_pk,
            96,
        )
        .await;

        assert!(
            !pending_requests().lock().unwrap().contains_key(trade_pk),
            "the late reply consumes the record"
        );
        let row = db
            .get_trade_by_order_id(&order_id)
            .await
            .expect("lookup")
            .expect("the payload-less late reply persists the trade from the snapshot");
        assert_eq!(row.role, TradeRole::Buyer, "the snapshot's role is kept");
        assert!(!row.order.is_mine, "buyer of a sell order is its taker");
        assert_eq!(
            row.order.status,
            crate::api::types::OrderStatus::WaitingPayment,
            "waiting-seller-to-pay implies waiting-payment, as the live waiter derives it"
        );
        assert_eq!(row.order.fiat_code, "EUR", "the snapshot's order data is kept");
        assert_eq!(row.trade_key_index, 96, "the durable key binding is kept");
        assert_wired_as_accepted_take(&order_id, 96, TradeRole::Buyer).await;
        assert_eq!(
            drain_updates(&mut rx, &order_id),
            vec![crate::api::types::OrderStatus::WaitingPayment],
            "the UI is pushed as part of landing the trade"
        );

        // Replayed (another relay's copy): no record left, no snapshot to
        // rebuild from, a row already holding the status — nothing changes.
        dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::WaitingSellerToPay,
                None,
                now as u64,
                Some(request_id),
            ),
            "test-late-wstp-replay",
            trade_pk,
            96,
        )
        .await;
        assert!(drain_updates(&mut rx, &order_id).is_empty());
        let row = db
            .get_trade_by_order_id(&order_id)
            .await
            .expect("lookup")
            .expect("row still there");
        assert_eq!(
            row.order.status,
            crate::api::types::OrderStatus::WaitingPayment
        );
    }

    /// #566, seller side: a seller taking a buy order is answered with
    /// `pay-invoice`, whose payload carries the hold invoice. Late, it must
    /// land the trade with that invoice — a rebuilt row without it is a
    /// trade the seller cannot pay.
    #[tokio::test]
    async fn a_late_pay_invoice_take_reply_lands_the_trade_with_its_hold_invoice() {
        // Current timestamps: a row this recent stays inside the stale
        // sweep's age gate, so a sweep another test runs meanwhile leaves it
        // alone.
        let now = crate::rt::unix_now();
        use mostro_core::message::{Action, Payload};
        let path = std::env::temp_dir().join(format!("mostro_late_pay_{}.db", std::process::id()));
        let _ = crate::db::app_db::init_db(path.to_str().unwrap()).await;
        let db = crate::db::app_db::db().expect("store initialised");

        let order_uuid = uuid::Uuid::new_v4();
        let order_id = order_uuid.to_string();
        let trade_pk = "ff00ff43";
        let request_id = 5_551_234_567u64;

        let (conf_tx, conf_rx) = tokio::sync::oneshot::channel::<Wake>();
        pending_requests().lock().unwrap().insert(
            trade_pk.to_string(),
            PendingRequest {
                request_id,
                trade_index: 97,
                kind: PendingRequestKind::Take {
                    order: Box::new(taken_book_order(
                        &order_id,
                        crate::api::types::OrderKind::Buy,
                    )),
                    role: TradeRole::Seller,
                    fiat_amount: None,
                },
                tx: Some(conf_tx),
            },
        );
        drop(conf_rx);
        detach_request_waiter(trade_pk, request_id);

        let so = mostro_core::order::SmallOrder::new(
            Some(order_uuid),
            Some(mostro_core::order::Kind::Buy),
            Some(mostro_core::order::Status::WaitingPayment),
            11_514,
            "EUR".to_string(),
            None,
            None,
            50,
            "SEPA".to_string(),
            0,
            None,
            None,
            None,
            None,
            None,
        );
        dispatch_mostro_message(
            daemon_message_with_nonce(
                order_uuid,
                Action::PayInvoice,
                Some(Payload::PaymentRequest(
                    Some(so),
                    "lnbcrt115140hold".to_string(),
                    Some(11_514),
                )),
                now as u64,
                Some(request_id),
            ),
            "test-late-pay-invoice",
            trade_pk,
            97,
        )
        .await;

        let row = db
            .get_trade_by_order_id(&order_id)
            .await
            .expect("lookup")
            .expect("the late reply persists the trade without a restart");
        assert_eq!(
            row.role,
            TradeRole::Seller,
            "pay-invoice addresses the seller"
        );
        assert_eq!(
            row.hold_invoice.as_deref(),
            Some("lnbcrt115140hold"),
            "the seller can pay the rebuilt trade"
        );
        assert_eq!(
            row.current_step,
            crate::api::types::TradeStep::Seller(crate::api::types::SellerStep::TakerFound),
            "the taker's first step, not the maker's OrderPublished"
        );
        assert_live_path_row(&row, &order_id, now);
        assert_eq!(
            row.order.status,
            crate::api::types::OrderStatus::WaitingPayment
        );
    }

    // ── The shipped registry, across runtime threads (#566 review) ──────────
    //
    // Every test above runs the registry on one thread. These run the type
    // the app ships — a `Mutex`, shared by tasks on different workers — each
    // over a registry of its own, leaked like the app's static, so no other
    // test's retries are in it. They drive `start_retry_task` and the
    // teardown's own halves, never a copy of them.

    fn own_registry() -> &'static RetryRegistry {
        Box::leak(Box::default())
    }

    /// Waits, boundedly, until every retry task in `registry` has finished
    /// and taken itself out.
    async fn until_no_retry_tasks(registry: &'static RetryRegistry) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !lock_retries(registry).tasks.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("every retry task finishes and deregisters");
    }

    /// The teardown waits for a retry caught in the middle of a poll on
    /// another worker. An abort only lands at a task's next poll, so without
    /// the wait the teardown would go on — and the next identity begin —
    /// while the old identity's retry still runs. The task blocks its worker
    /// synchronously until released: mid-poll, not parked at an await, which
    /// is the one case an abort alone does not stop.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_teardown_waits_for_a_retry_caught_mid_poll_on_another_worker() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let registry = own_registry();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let ended = std::sync::Arc::new(AtomicBool::new(false));
        let ended_in_task = std::sync::Arc::clone(&ended);
        let scheduled = start_retry_task(registry, 0, "test-mt-mid-poll", || {}, |_| {
            Box::pin(async move {
                entered_tx.send(()).expect("the test listens");
                // Blocks this worker: the task is inside a poll.
                release_rx.recv().expect("the test releases it");
                ended_in_task.store(true, SeqCst);
            })
        });
        assert!(matches!(scheduled, Scheduled::Attempt { attempt: 0, .. }));
        tokio::task::spawn_blocking(move || entered_rx.recv().expect("the retry starts"))
            .await
            .expect("the wait for the retry");

        let tasks = lock_retries(registry).begin_forget();
        let teardown = tokio::spawn(stop_retry_tasks(tasks));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !teardown.is_finished(),
            "the teardown is still waiting while the retry runs"
        );
        release_tx.send(()).expect("the retry is waiting");
        tokio::time::timeout(std::time::Duration::from_secs(5), teardown)
            .await
            .expect("the teardown returns once the retry has stopped")
            .expect("the teardown task");
        assert!(
            ended.load(SeqCst),
            "the poll the abort caught ran to its end — which is why the wait is needed"
        );
        assert!(lock_retries(registry).tasks.is_empty());
    }

    /// The teardown stops a retry parked at an await at once — aborted, not
    /// waited out — and the retry never runs past that await.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_teardown_aborts_a_retry_parked_at_an_await_on_another_worker() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let registry = own_registry();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
        let resumed = std::sync::Arc::new(AtomicBool::new(false));
        let resumed_in_task = std::sync::Arc::clone(&resumed);
        let scheduled = start_retry_task(registry, 0, "test-mt-parked", || {}, |_| {
            Box::pin(async move {
                let _ = entered_tx.send(());
                std::future::pending::<()>().await;
                resumed_in_task.store(true, SeqCst);
            })
        });
        assert!(matches!(scheduled, Scheduled::Attempt { attempt: 0, .. }));
        entered_rx.await.expect("the retry starts");

        let tasks = lock_retries(registry).begin_forget();
        tokio::time::timeout(std::time::Duration::from_secs(5), stop_retry_tasks(tasks))
            .await
            .expect("a parked retry is aborted, not waited out");
        assert!(!resumed.load(SeqCst), "an aborted retry is never polled again");
        let retries = lock_retries(registry);
        assert!(retries.tasks.is_empty());
        assert_eq!(retries.epoch, 1, "the teardown closed the epoch");
    }

    /// The bookkeeping, with each attempt's task running on a worker: a take
    /// gets one attempt per delay and is then given up, its record put back
    /// only for an attempt that is due; a settled take starts counting
    /// again; and once a teardown closes the epoch, a dispatch that began
    /// under the old one schedules nothing and puts nothing back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_shipped_registry_counts_settles_and_closes_epochs_across_workers() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let registry = own_registry();
        let put_backs = AtomicUsize::new(0);
        let put_back = || {
            put_backs.fetch_add(1, SeqCst);
        };
        let finished = || -> RedispatchFuture<'static> { Box::pin(async {}) };

        for expected in 0..LATE_TAKE_RETRY_DELAYS.len() {
            let scheduled = start_retry_task(registry, 0, "test-mt-take", put_back, |_| finished());
            assert!(
                matches!(scheduled, Scheduled::Attempt { attempt, .. } if attempt == expected),
                "attempt {expected} is due"
            );
            until_no_retry_tasks(registry).await;
        }
        let scheduled = start_retry_task(registry, 0, "test-mt-take", put_back, |_| finished());
        assert!(
            matches!(
                scheduled,
                Scheduled::Exhausted { attempts } if attempts == LATE_TAKE_RETRY_DELAYS.len() + 1
            ),
            "past the last delay the take is given up"
        );
        assert_eq!(put_backs.load(SeqCst), LATE_TAKE_RETRY_DELAYS.len());
        assert!(
            !lock_retries(registry).attempts.contains_key("test-mt-take"),
            "a given-up take leaves no bookkeeping"
        );

        let scheduled = start_retry_task(registry, 0, "test-mt-settled", put_back, |_| finished());
        assert!(matches!(scheduled, Scheduled::Attempt { attempt: 0, .. }));
        until_no_retry_tasks(registry).await;
        lock_retries(registry).settle("test-mt-settled");
        let scheduled = start_retry_task(registry, 0, "test-mt-settled", put_back, |_| finished());
        assert!(
            matches!(scheduled, Scheduled::Attempt { attempt: 0, .. }),
            "a settled take counts from zero again"
        );
        until_no_retry_tasks(registry).await;

        let tasks = lock_retries(registry).begin_forget();
        stop_retry_tasks(tasks).await;
        let before = put_backs.load(SeqCst);
        let scheduled =
            start_retry_task(registry, 0, "test-mt-old-epoch", put_back, |_| finished());
        assert!(matches!(scheduled, Scheduled::IdentityGone));
        assert_eq!(put_backs.load(SeqCst), before, "nothing put back for a gone identity");
        let retries = lock_retries(registry);
        assert!(retries.tasks.is_empty() && !retries.attempts.contains_key("test-mt-old-epoch"));
    }
}
