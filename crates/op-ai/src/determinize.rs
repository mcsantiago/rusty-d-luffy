//! Determinization: turning an imperfect-information position into a concrete
//! one an ordinary search can handle.
//!
//! A searching agent must not read cards it cannot see. Rather than
//! reconstructing a state from a [`op_core::view::PlayerView`], we take the
//! true state and **destroy** what the observer is not entitled to, by
//! reshuffling every hidden card among the hidden positions it could occupy.
//!
//! # What this hides, and what it does not
//!
//! Reshuffling destroys *where* each hidden card is — in hand, or buried in
//! the deck. It preserves the *multiset* of hidden cards, because it only
//! permutes identities that are already there.
//!
//! Preserving the multiset is sound **only when the observer knows the
//! opponent's decklist**. Then `hand ∪ deck ∪ life` is exactly
//! `decklist − revealed`, which the observer could work out unaided. That holds
//! in this app, where both decks are chosen from a fixed list and the player
//! picks the AI's, and in competitive play, where decklists are submitted.
//!
//! It does **not** hold against an unknown or custom deck — the normal case
//! once there is a deckbuilder or multiplayer. There the agent gets the
//! opponent's exact composition for free from turn zero, including how many
//! counters remain to be drawn, which is enough on its own to change whether an
//! attack is correct. This is a known limitation, not a subtlety.
//!
//! The honest fix is a prior over decklists: sample an archetype consistent
//! with what has been revealed, then deal the hidden zones from it. Sampling
//! uniformly from every card legal with the opponent's Leader colours
//! (5-1-2-2) would remove the advantage but not be an improvement — hands drawn
//! from several hundred candidates are noise, and search quality would collapse
//! below the over-informed version this has today.

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
///
/// Because it shuffles identities already present rather than drawing fresh
/// ones, the multiset across `zones` survives untouched — the assumption
/// discussed in the module docs.
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
    use rand::SeedableRng;
    use std::collections::BTreeMap;

    fn multiset(state: &GameState, player: PlayerId, zone: Zone) -> BTreeMap<CardDefId, usize> {
        let mut counts = BTreeMap::new();
        for &id in state.player(player).zone(zone) {
            *counts.entry(state.card(id).def).or_insert(0) += 1;
        }
        counts
    }

    /// A state with cards in every zone that matters, built without card data
    /// so this runs on a bare clone.
    fn scratch_state() -> (op_core::card::CardDb, GameState) {
        use op_core::card::{CardDef, Category, CardDb};

        let mut db = CardDb::empty();
        for i in 0..6 {
            db.insert(CardDef {
                number: format!("T-{i:03}"),
                name: format!("Test {i}"),
                category: Category::Character,
                colors: Vec::new(),
                cost: 1,
                life: None,
                power: Some(1000),
                counter: None,
                types: Vec::new(),
                attributes: Vec::new(),
                keywords: Vec::new(),
                effect: None,
                trigger: None,
            });
        }

        let mut state = GameState::new(1, PlayerId::P0);
        for player in [PlayerId::P0, PlayerId::P1] {
            // Distinct identities spread across hidden and open zones, so a
            // shuffle that reached the wrong zone would be visible.
            for (i, zone) in [
                Zone::Hand, Zone::Hand, Zone::Hand,
                Zone::Deck, Zone::Deck, Zone::Deck,
                Zone::Life, Zone::Life,
                Zone::Character, Zone::Trash,
            ]
            .into_iter()
            .enumerate()
            {
                let def = db.by_number(&format!("T-{:03}", i % 6)).unwrap();
                state.spawn(def, player, zone);
            }
        }
        (db, state)
    }

    #[test]
    fn determinization_leaves_everything_observable_untouched() {
        let (_db, before) = scratch_state();
        let mut after = before.clone();
        let mut rng = rand::rngs::StdRng::seed_from_u64(4);
        determinize(&mut after, PlayerId::P0, &mut rng);

        assert_consistent(&before, &after, PlayerId::P0);
    }

    /// The point of the exercise: the opponent's hand must actually move.
    /// A determinizer that quietly did nothing would pass every consistency
    /// check above while leaking the whole hand.
    #[test]
    fn determinization_actually_scrambles_the_opponents_hand() {
        let (_db, before) = scratch_state();
        let opponent = PlayerId::P1;

        let hand_defs = |s: &GameState| -> Vec<_> {
            s.player(opponent)
                .zone(Zone::Hand)
                .iter()
                .map(|&id| s.card(id).def)
                .collect()
        };

        // Any single shuffle can land on the identity permutation, so this
        // asks whether it ever moves across several seeds.
        let moved = (0..25).any(|seed| {
            let mut after = before.clone();
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            determinize(&mut after, opponent.opponent(), &mut rng);
            hand_defs(&after) != hand_defs(&before)
        });
        assert!(moved, "the opponent's hand was never redistributed");
    }

    /// Determinization must not change anything observable: zone sizes stay
    /// put, and open areas keep their exact contents.
    fn assert_consistent(before: &GameState, after: &GameState, observer: PlayerId) {
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
