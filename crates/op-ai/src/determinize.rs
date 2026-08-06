//! Determinization: turning an imperfect-information position into a concrete
//! one an ordinary search can handle.
//!
//! A searching agent must not read cards it cannot see. Rather than trying to
//! reconstruct a state from a [`op_core::view::PlayerView`], we take the true
//! state and **destroy** the information the observer is not entitled to, by
//! reshuffling every hidden card among the hidden positions it could occupy.
//! The result is consistent with everything the observer knows and independent
//! of what they do not, which is exactly the guarantee a determinization needs.

use rand::seq::SliceRandom;
use rand::Rng;

use op_core::ids::{CardDefId, PlayerId};
use op_core::state::GameState;
use op_core::zone::Zone;

/// Reshuffles hidden information from `observer`'s point of view.
///
/// Hidden to the observer:
///   * their own deck and Life area — they know the multiset, not the order
///     (3-2-2, 3-1-4)
///   * the opponent's hand, deck and Life area (3-4-2, 3-2-2)
///
/// Zone membership, counts, and every open area are left exactly as they are,
/// so the position stays legal and consistent.
pub fn determinize(state: &mut GameState, observer: PlayerId, rng: &mut impl Rng) {
    // The observer's own concealed cards: they know what is in there, but not
    // where, so deck and Life are permuted together.
    redistribute(state, observer, &[Zone::Deck, Zone::Life], rng);

    // The opponent's concealed cards: hand, deck and Life are mutually
    // indistinguishable to the observer.
    redistribute(
        state,
        observer.opponent(),
        &[Zone::Hand, Zone::Deck, Zone::Life],
        rng,
    );
}

/// Permutes which card *identity* sits in each hidden slot, leaving the slots
/// themselves untouched.
fn redistribute(state: &mut GameState, player: PlayerId, zones: &[Zone], rng: &mut impl Rng) {
    let mut slots: Vec<op_core::ids::CardInstanceId> = Vec::new();
    for &zone in zones {
        slots.extend(state.player(player).zone(zone).iter().copied());
    }
    if slots.len() < 2 {
        return;
    }

    let mut defs: Vec<CardDefId> = slots.iter().map(|&id| state.card(id).def).collect();
    defs.shuffle(rng);

    // Swapping only the printed identity keeps every id, zone list and index
    // stable — nothing else in the state can notice the shuffle.
    for (&slot, def) in slots.iter().zip(defs) {
        state.card_mut(slot).def = def;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn multiset(state: &GameState, player: PlayerId, zone: Zone) -> BTreeMap<CardDefId, usize> {
        let mut counts = BTreeMap::new();
        for &id in state.player(player).zone(zone) {
            *counts.entry(state.card(id).def).or_insert(0) += 1;
        }
        counts
    }

    /// Determinization must not change anything observable: zone sizes stay
    /// put, and open areas keep their exact contents.
    pub fn assert_consistent(before: &GameState, after: &GameState, observer: PlayerId) {
        for player in [PlayerId::P0, PlayerId::P1] {
            for zone in [
                Zone::Deck,
                Zone::Hand,
                Zone::Life,
                Zone::Character,
                Zone::Trash,
                Zone::Cost,
            ] {
                assert_eq!(
                    before.player(player).zone(zone).len(),
                    after.player(player).zone(zone).len(),
                    "zone size changed for {player:?} {zone:?}"
                );
                // Open areas must be untouched.
                if zone.is_open() {
                    assert_eq!(
                        multiset(before, player, zone),
                        multiset(after, player, zone),
                        "open area {zone:?} was altered"
                    );
                }
            }
        }
        // The observer's own hand is known to them and must survive intact.
        assert_eq!(
            multiset(before, observer, Zone::Hand),
            multiset(after, observer, Zone::Hand)
        );
    }
}
