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

/// Record the info `event` a capability fetch brought for its author.
pub(crate) fn set_from_event(event: &Event) {
    set_live(
        &event.pubkey.to_hex(),
        CachedNodeInfo {
            created_at: event.created_at.as_secs() as i64,
            event_id: Some(event.id.to_hex()),
            tags: event.tags.iter().map(|t| t.as_slice().to_vec()).collect(),
        },
    );
}

/// Record what `node` announces now, as a revision dated this instant with
/// no id: newer than anything cached, older than any later fetch.
#[cfg(test)]
pub(crate) fn set_from_tags(node: &str, tags: &[Vec<String>]) {
    set_live(
        node,
        CachedNodeInfo {
            created_at: crate::rt::unix_now(),
            event_id: None,
            tags: tags.to_vec(),
        },
    );
}

fn set_live(node: &str, info: CachedNodeInfo) {
    LIVE.write()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(node.to_lowercase(), info);
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
