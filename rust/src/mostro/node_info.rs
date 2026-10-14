//! The newest kind 38385 info event this app holds for a node.
//!
//! Two sources, and the newest revision by NIP-01's order decides, whichever
//! source holds it:
//! - every registry node's kind 38385 cached by `node_stats`, so a node the
//!   user switched away from is still described;
//! - the event the node's last capability fetch brought, kept here per node.
//!   The cache write is best effort, so a fetch the store could not keep is
//!   still the newest word; and a node the store never saw has only this.
//!
//! Neither source retracts the other by being read later: a fetch can end
//! empty (every relay slow at startup) or carry a stale copy from a relay
//! that is behind. A retraction of a tag is a newer event without it.

use std::collections::HashMap;
use std::sync::RwLock;

use nostr_sdk::prelude::Event;

use crate::api::node_stats::CachedNodeInfo;

/// Per node (hex), the info event its last capability fetch brought.
static LIVE: RwLock<Option<HashMap<String, CachedNodeInfo>>> = RwLock::new(None);

/// Record the info `event` a capability fetch brought for its author, unless
/// a newer revision is already held: a fetch answered by a relay that is
/// behind must not undo a newer one.
pub(crate) fn set_from_event(event: &Event) {
    let info = CachedNodeInfo {
        created_at: event.created_at.as_secs() as i64,
        event_id: Some(event.id.to_hex()),
        tags: event.tags.iter().map(|t| t.as_slice().to_vec()).collect(),
    };
    let mut live = LIVE.write().unwrap_or_else(|e| e.into_inner());
    keep_newest(
        live.get_or_insert_with(HashMap::new),
        &event.pubkey.to_hex(),
        info,
    );
}

/// Hold `info` for `node` unless what `held` has for it supersedes it.
fn keep_newest(held: &mut HashMap<String, CachedNodeInfo>, node: &str, info: CachedNodeInfo) {
    let node = node.to_lowercase();
    if held.get(&node).is_some_and(|prev| !info.supersedes(prev)) {
        return;
    }
    held.insert(node, info);
}

/// Record what `node` announces now, as a revision dated this instant with
/// no id, replacing whatever is held: a test's word is the newest.
#[cfg(test)]
pub(crate) fn set_from_tags(node: &str, tags: &[Vec<String>]) {
    LIVE.write()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(
            node.to_lowercase(),
            CachedNodeInfo {
                created_at: crate::rt::unix_now(),
                event_id: None,
                tags: tags.to_vec(),
            },
        );
}

/// What the capability fetches brought, per node.
pub(crate) fn live() -> HashMap<String, CachedNodeInfo> {
    LIVE.read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

/// `node`'s newest revision among `live` and `cached` (NIP-01's order), node
/// keys compared case-insensitively.
pub(crate) fn newest_in(
    node: &str,
    live: &HashMap<String, CachedNodeInfo>,
    cached: &HashMap<String, CachedNodeInfo>,
) -> Option<CachedNodeInfo> {
    let node = node.trim().to_lowercase();
    let of = |source: &HashMap<String, CachedNodeInfo>| {
        source
            .iter()
            .find(|(source_node, _)| source_node.to_lowercase() == node)
            .map(|(_, info)| info.clone())
    };
    match (of(live), of(cached)) {
        (Some(live), Some(cached)) if live.supersedes(&cached) => Some(live),
        (_, Some(cached)) => Some(cached),
        (live, None) => live,
    }
}

/// `node`'s (hex) newest info event this app holds, from either source.
pub(crate) async fn newest(node: &str) -> Option<CachedNodeInfo> {
    let cached = crate::api::node_stats::cached_info().await;
    newest_in(node, &live(), &cached)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision(created_at: i64, id: &str) -> CachedNodeInfo {
        CachedNodeInfo {
            created_at,
            event_id: Some(id.to_string()),
            tags: vec![vec![
                "hold_invoice_cltv_delta".into(),
                created_at.to_string(),
            ]],
        }
    }

    /// A fetch answered by a relay that is behind brings an older revision:
    /// it must not undo the newer one already held.
    #[test]
    fn an_older_fetch_does_not_replace_a_newer_one() {
        let mut held = HashMap::new();
        keep_newest(&mut held, "NODE", revision(2_000, "bb"));
        keep_newest(&mut held, "node", revision(1_000, "aa"));
        assert_eq!(held["node"].created_at, 2_000);

        keep_newest(&mut held, "node", revision(3_000, "cc"));
        assert_eq!(held["node"].created_at, 3_000);
        assert_eq!(held.len(), 1, "node keys are case-insensitive");
    }

    /// Within one second NIP-01 keeps the lowest id, as the cache does.
    #[test]
    fn a_tie_goes_to_the_lowest_id() {
        let mut held = HashMap::new();
        keep_newest(&mut held, "node", revision(2_000, "bb"));
        keep_newest(&mut held, "node", revision(2_000, "cc"));
        assert_eq!(held["node"].event_id.as_deref(), Some("bb"));
        keep_newest(&mut held, "node", revision(2_000, "aa"));
        assert_eq!(held["node"].event_id.as_deref(), Some("aa"));
    }
}
