//! How long after the escrow locks a node's deadline guardian acts on a trade.
//!
//! Since mostrod v0.18.3 (mostro#862) the guardian acts once the chain tip is
//! `escrow_deadline_margin_blocks` from the seller's hold invoice expiry: an
//! `active` trade is cancelled, a `fiat-sent` one goes to dispute. Counted
//! from the moment the escrow locked, that is about
//! `(hold_invoice_cltv_delta - escrow_deadline_margin_blocks)` blocks of ten
//! minutes. Both come from the node's info event (kind 38385); the margin
//! since mostro#1057, which also refuses to start a node whose margin is not
//! in `18..hold_invoice_cltv_delta`.
//!
//! An estimate, never an exact time: blocks are not ten minutes each, and
//! the HTLC's real expiry is a few blocks later than the delta (the paying
//! node pads it), so the guardian acts somewhat after the estimate.
//!
//! No window — so no countdown — when the node runs Cashu (no hold invoice,
//! no guardian), predates the guardian, or advertises values the daemon
//! would not run with.

/// Seconds per block the estimate assumes.
pub const SECS_PER_BLOCK: i64 = 600;

/// mostrod's default `escrow_deadline_margin_blocks`, assumed for a node on
/// v0.18.3 or later that does not publish it.
pub const DEFAULT_MARGIN_BLOCKS: u32 = 24;

/// The smallest margin mostrod starts with (mostro#1057): LND's default
/// `holdexpirydelta` (12) plus 6 blocks of headroom.
pub const MIN_MARGIN_BLOCKS: u32 = 18;

/// The first mostrod with the deadline guardian (mostro#862).
const GUARDIAN_SINCE: semver::Version = semver::Version::new(0, 18, 3);

/// What a node's info event says about its escrow deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EscrowWindow {
    pub cltv_delta_blocks: u32,
    pub margin_blocks: u32,
    /// The node does not publish the margin: the default is assumed.
    pub margin_assumed: bool,
}

impl EscrowWindow {
    /// Seconds from the escrow lock to the guardian's action, estimated.
    pub fn secs(&self) -> i64 {
        i64::from(self.cltv_delta_blocks - self.margin_blocks) * SECS_PER_BLOCK
    }
}

/// The escrow window an info event's tags describe, or `None` when the node
/// has no guardian or advertises values it would not run with.
pub fn parse_tags(tags: &[Vec<String>]) -> Option<EscrowWindow> {
    let value = |name: &str| {
        tags.iter()
            .find(|tag| tag.first().map(String::as_str) == Some(name))
            .and_then(|tag| tag.get(1))
            .map(|value| value.trim())
    };
    if value("escrow_mode")
        .is_some_and(|mode| crate::mostro::escrow_mode::EscrowMode::from_tag_value(mode).is_cashu())
    {
        return None;
    }
    let cltv_delta_blocks = value("hold_invoice_cltv_delta")?.parse::<u32>().ok()?;
    let (margin_blocks, margin_assumed) = match value("escrow_deadline_margin_blocks") {
        Some(margin) => (margin.parse::<u32>().ok()?, false),
        None => {
            let version = value("mostro_version")?;
            let version = semver::Version::parse(version.trim_start_matches('v')).ok()?;
            if version < GUARDIAN_SINCE {
                return None;
            }
            (DEFAULT_MARGIN_BLOCKS, true)
        }
    };
    (MIN_MARGIN_BLOCKS..cltv_delta_blocks)
        .contains(&margin_blocks)
        .then_some(EscrowWindow {
            cltv_delta_blocks,
            margin_blocks,
            margin_assumed,
        })
}

/// The escrow window of `node` (hex), from the newest info event this app
/// holds for it.
pub async fn for_node(node: &str) -> Option<EscrowWindow> {
    crate::mostro::node_info::newest(node)
        .await
        .and_then(|info| parse_tags(&info.tags))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> Vec<Vec<String>> {
        pairs
            .iter()
            .map(|(name, value)| vec![name.to_string(), value.to_string()])
            .collect()
    }

    #[test]
    fn a_published_margin_is_used() {
        let window = parse_tags(&tags(&[
            ("mostro_version", "0.19.0"),
            ("hold_invoice_cltv_delta", "144"),
            ("escrow_deadline_margin_blocks", "48"),
        ]))
        .expect("a window");
        assert_eq!(
            window,
            EscrowWindow {
                cltv_delta_blocks: 144,
                margin_blocks: 48,
                margin_assumed: false,
            }
        );
        assert_eq!(window.secs(), 96 * 600);
    }

    #[test]
    fn a_node_with_the_guardian_but_no_margin_tag_is_assumed_at_the_default() {
        for version in ["0.18.3", "0.18.8", "v0.18.8", "1.0.0"] {
            let window = parse_tags(&tags(&[
                ("mostro_version", version),
                ("hold_invoice_cltv_delta", "144"),
            ]))
            .unwrap_or_else(|| panic!("a window for {version}"));
            assert_eq!(window.margin_blocks, DEFAULT_MARGIN_BLOCKS);
            assert!(window.margin_assumed);
            assert_eq!(window.secs(), 120 * 600, "about 20 h on the defaults");
        }
    }

    #[test]
    fn a_node_without_the_guardian_or_an_unknown_one_has_no_window() {
        for version in ["0.18.2", "0.17.0", "0.18.3-rc1", "not-a-version"] {
            assert_eq!(
                parse_tags(&tags(&[
                    ("mostro_version", version),
                    ("hold_invoice_cltv_delta", "144"),
                ])),
                None,
                "{version}"
            );
        }
        assert_eq!(
            parse_tags(&tags(&[("hold_invoice_cltv_delta", "144")])),
            None
        );
    }

    #[test]
    fn values_the_daemon_would_not_run_with_give_no_window() {
        for (cltv, margin) in [
            ("144", "17"),
            ("144", "144"),
            ("144", "1000"),
            ("144", "x"),
            ("x", "24"),
            ("0", "24"),
        ] {
            assert_eq!(
                parse_tags(&tags(&[
                    ("mostro_version", "0.19.0"),
                    ("hold_invoice_cltv_delta", cltv),
                    ("escrow_deadline_margin_blocks", margin),
                ])),
                None,
                "cltv {cltv}, margin {margin}"
            );
        }
        assert_eq!(
            parse_tags(&tags(&[
                ("mostro_version", "0.19.0"),
                ("escrow_deadline_margin_blocks", "24"),
            ])),
            None,
            "no cltv delta"
        );
        // Both bounds of the range the daemon starts with.
        for margin in ["18", "143"] {
            assert!(parse_tags(&tags(&[
                ("hold_invoice_cltv_delta", "144"),
                ("escrow_deadline_margin_blocks", margin),
            ]))
            .is_some());
        }
    }

    #[test]
    fn a_cashu_node_has_no_window() {
        assert_eq!(
            parse_tags(&tags(&[
                ("mostro_version", "0.19.0"),
                ("escrow_mode", "cashu"),
                ("hold_invoice_cltv_delta", "144"),
                ("escrow_deadline_margin_blocks", "24"),
            ])),
            None
        );
        assert!(parse_tags(&tags(&[
            ("escrow_mode", "lightning"),
            ("hold_invoice_cltv_delta", "144"),
            ("escrow_deadline_margin_blocks", "24"),
        ]))
        .is_some());
    }

    /// The window is read from the newest revision this app holds for the
    /// node, the live one included.
    #[tokio::test]
    async fn the_window_of_a_node_comes_from_its_newest_info_event() {
        let node = "aa".repeat(32);
        assert_eq!(for_node(&node).await, None);
        crate::mostro::node_info::set_from_tags(
            &node,
            &tags(&[
                ("hold_invoice_cltv_delta", "298"),
                ("escrow_deadline_margin_blocks", "24"),
            ]),
        );
        assert_eq!(
            for_node(&node).await.map(|window| window.secs()),
            Some(274 * 600)
        );
    }
}
