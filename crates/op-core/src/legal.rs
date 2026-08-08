//! Legal action enumeration.
//!
//! One function serves three consumers: the server validates incoming actions
//! against it, the RL env uses it as an action mask, and search agents use it
//! as the move generator. Keeping them on the same implementation is what stops
//! the three from disagreeing about what is legal.

use crate::action::{Action, Pending};
use crate::card::Category;
use crate::derive;
use crate::game::Game;
use crate::ids::CardInstanceId;
use crate::zone::Zone;

/// Cap on enumerated subsets for a "choose up to N" request. Beyond this the
/// list is truncated rather than exploding; no printed card comes close.
const MAX_CHOICE_SUBSETS: usize = 256;

/// Every action legal in the current position.
///
/// Empty exactly when the game is over or nothing is pending.
pub fn legal_actions(game: &Game) -> Vec<Action> {
    let Some(pending) = game.pending() else {
        return Vec::new();
    };
    if game.is_over() {
        return Vec::new();
    }

    match pending {
        Pending::Mulligan { .. } => vec![Action::Mulligan(false), Action::Mulligan(true)],

        Pending::MainAction { player } => {
            let player = *player;
            let state = &game.state;
            let derived = game.derived();
            let mut out = vec![Action::EndMainPhase];

            // Play a card (6-5-3-1).
            let affordable = game.active_don(player).len();
            let characters_full = state.player(player).characters.len() >= 5;
            for &card in &state.player(player).hand {
                let def = game.db().get(state.card(card).def);
                if def.cost as usize > affordable {
                    continue;
                }
                match def.category {
                    // 3-7-6-1: with a full board, every Character already out
                    // is a legal thing to trash for room, so the choice is
                    // enumerated rather than the play being refused.
                    Category::Character if characters_full => {
                        for victim in state.player(player).characters.clone() {
                            out.push(Action::PlayCard {
                                card,
                                replacing: Some(victim),
                            });
                        }
                        continue;
                    }
                    Category::Character | Category::Stage => {}
                    // An Event with no [Main] effect has nothing to do when
                    // played (its text may be [Counter]-only).
                    Category::Event => {
                        if game
                            .scripts()
                            .script(state.card(card).def)
                            .activated
                            .is_empty()
                        {
                            continue;
                        }
                    }
                    Category::Leader | Category::Don => continue,
                }
                out.push(Action::PlayCard {
                    card,
                    replacing: None,
                });
            }

            // Activate an effect on a card in play (6-5-4-1).
            for card in state
                .player(player)
                .zone(Zone::Character)
                .to_vec()
                .into_iter()
                .chain(state.player(player).leader)
                .chain(state.player(player).stage)
            {
                for effect in &game.scripts().script(state.card(card).def).activated {
                    if effect.once_per_turn
                        && state.card(card).used_once_per_turn.contains(&effect.slot)
                    {
                        continue;
                    }
                    if !derive::conditions_hold(state, game.db(), &[], card, &effect.conditions) {
                        continue;
                    }
                    if !game.can_pay(player, card, &effect.cost) {
                        continue;
                    }
                    // A hand cost is a choice: which cards to trash is the
                    // player's, so each option is offered separately.
                    for discard in subsets_of_size(
                        state.player(player).hand.as_slice(),
                        effect.cost.trash_from_hand as usize,
                    ) {
                        out.push(Action::ActivateEffect {
                            card,
                            slot: effect.slot,
                            discard,
                        });
                    }
                }
            }

            // Give DON!! (6-5-5-1).
            if affordable > 0 {
                for to in state.battlers(player) {
                    out.push(Action::GiveDon { to });
                }
            }

            // Declare an attack (7-1).
            for attacker in state.battlers(player) {
                if !derive::can_attack(state, game.db(), &derived, attacker) {
                    continue;
                }
                for target in derive::attack_targets(state, attacker) {
                    out.push(Action::Attack { attacker, target });
                }
            }

            out
        }

        Pending::Block { .. } => {
            let mut out = vec![Action::Block { blocker: None }];
            out.extend(
                game.legal_blockers()
                    .into_iter()
                    .map(|b| Action::Block { blocker: Some(b) }),
            );
            out
        }

        Pending::Counter { player } => {
            let player = *player;
            let mut out = vec![Action::DoneCountering];
            let boostable: Vec<CardInstanceId> = game
                .state
                .battlers(player)
                .into_iter()
                .filter(|&c| matches!(game.state.card(c).zone, Zone::Leader | Zone::Character))
                .collect();
            for card in game.legal_counters() {
                for &to in &boostable {
                    out.push(Action::Counter { card, to });
                }
            }
            for card in game.legal_counter_events() {
                for &to in &boostable {
                    out.push(Action::CounterEvent { card, to });
                }
            }
            out
        }

        Pending::Trigger { .. } => vec![Action::UseTrigger(false), Action::UseTrigger(true)],

        Pending::Choose { options, up_to, .. } => subsets(options, *up_to as usize)
            .into_iter()
            .map(|cards| Action::Choose { cards })
            .collect(),
    }
}

/// All subsets of `options` with exactly `n` elements.
///
/// `n == 0` yields one empty choice, which is what an effect with no hand cost
/// needs: exactly one way to pay nothing.
fn subsets_of_size(options: &[CardInstanceId], n: usize) -> Vec<Vec<CardInstanceId>> {
    if n == 0 {
        return vec![Vec::new()];
    }
    subsets(options, n)
        .into_iter()
        .filter(|s| s.len() == n)
        .collect()
}

/// All subsets of `options` with at most `max` elements, smallest first.
fn subsets(options: &[CardInstanceId], max: usize) -> Vec<Vec<CardInstanceId>> {
    let mut out = vec![Vec::new()];
    let mut frontier: Vec<Vec<CardInstanceId>> = vec![Vec::new()];

    for _ in 0..max {
        let mut next = Vec::new();
        for base in &frontier {
            // Only extend with options after the last one taken, so each
            // combination is produced once.
            let start = base
                .last()
                .and_then(|last| options.iter().position(|o| o == last).map(|i| i + 1))
                .unwrap_or(0);
            for &opt in &options[start.min(options.len())..] {
                let mut combo = base.clone();
                combo.push(opt);
                next.push(combo);
            }
        }
        if next.is_empty() {
            break;
        }
        out.extend(next.iter().cloned());
        frontier = next;
        if out.len() >= MAX_CHOICE_SUBSETS {
            out.truncate(MAX_CHOICE_SUBSETS);
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u32) -> Vec<CardInstanceId> {
        (0..n).map(CardInstanceId).collect()
    }

    #[test]
    fn subsets_are_unique_and_bounded_by_max() {
        let got = subsets(&ids(4), 2);
        // 1 empty + 4 singletons + 6 pairs
        assert_eq!(got.len(), 11);
        let mut sorted = got.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), got.len(), "subsets must be unique");
        assert!(got.iter().all(|s| s.len() <= 2));
    }

    #[test]
    fn subsets_of_zero_is_just_the_empty_choice() {
        assert_eq!(subsets(&ids(3), 0), vec![Vec::new()]);
    }
}
