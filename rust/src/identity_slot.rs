//! The identity slot: crate-internal, so it lives outside `crate::api`, where
//! the bridge codegen would scan it. The transitions that use it are in
//! `api::identity`.

use nostr_sdk::prelude::Keys;
use tokio::sync::RwLock;

use crate::api::types::IdentityInfo;

/// The identity loaded into the core: its words, its keys and what the UI
/// reads of it.
pub(crate) struct IdentityState {
    pub(crate) mnemonic_words: Vec<String>,
    pub(crate) keys: Keys,
    pub(crate) identity_info: IdentityInfo,
}

/// The loaded identity and the generation that tells it from the next one.
///
/// The process has one (`api::identity::identity_slot`); the lifecycle seams
/// (`create_in`, `load_in`, `import_in`, `import_nsec_in`, `delete_in`) take
/// it as a parameter so a test can drive them on a slot of its own, without
/// racing the tests that share the global one.
pub(crate) struct IdentitySlot {
    pub(crate) state: RwLock<Option<IdentityState>>,
    /// Bumped by every identity deletion, under the write lock: it tells work
    /// that started under one identity apart from the next — even when the
    /// same mnemonic is imported again, which a pubkey comparison would not.
    pub(crate) generation: std::sync::atomic::AtomicU64,
}

impl IdentitySlot {
    pub(crate) const fn new() -> Self {
        Self {
            state: RwLock::const_new(None),
            generation: std::sync::atomic::AtomicU64::new(0),
        }
    }
}
