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
                    if !derive::conditions_hold(
                        state,
                        game.db(),
                        &[],
                        card,
                        None,
                        &effect.conditions,
                    ) {
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

        // 8-3-1-4: both answers are always legal. Affordability was settled
        // before the question was asked.
        Pending::PayCost { .. } => vec![Action::PayCost(true), Action::PayCost(false)],

        // 8-3-1-6: every way of making up the cost. The count is fixed, so
        // unlike a `Choose` there is no "take fewer" answer.
        Pending::ReturnDon { options, n, .. } => return_don_actions(game, options, *n),

        Pending::Choose {
            options,
            up_to,
            at_least,
            ..
        } => choose_actions(options, *up_to, *at_least),
    }
}

/// What makes one DON!! different from another for the purpose of choosing:
/// the card it was given to, if any, and whether it is rested.
type DonClass = (Option<CardInstanceId>, bool);

/// Every legal answer to a `ReturnDon`, one per *distinguishable* answer.
///
/// DON!! cards are interchangeable. Two rested DON!! in the cost area differ
/// only by id: return either and the position is the same, down to the state
/// hash. Enumerating subsets of ids would offer ST04-001's `DON!! −7` over ten
/// DON!! as C(10,7) = 120 answers where there are two or three real ones, and
/// hand a search 120 branches that all lead to the same node — diluting its
/// statistics 40-fold over a decision that barely matters.
///
/// What distinguishes a DON!! is where it sits: loose in the cost area and
/// whether it is rested, or given to a particular card. So the answers are
/// enumerated over those classes, one representative per combination of counts.
/// Classes are taken greedily-first, which makes the leading answer the rested
/// cost-area DON!! — the same one the engine used to take without asking.
fn return_don_actions(game: &Game, options: &[CardInstanceId], n: u8) -> Vec<Action> {
    let n = (n as usize).min(options.len());

    // Grouped in the pool's order, so the enumeration is deterministic.
    let mut classes: Vec<(DonClass, Vec<CardInstanceId>)> = Vec::new();
    for &don in options {
        let key = (game.don_holder(don), game.state.card(don).rested);
        match classes.iter_mut().find(|(k, _)| *k == key) {
            Some((_, group)) => group.push(don),
            None => classes.push((key, vec![don])),
        }
    }
    let groups: Vec<Vec<CardInstanceId>> = classes.into_iter().map(|(_, g)| g).collect();

    let mut out = Vec::new();
    distribute(&groups, n, 0, &mut Vec::new(), &mut out);
    if out.is_empty() {
        out.push(Action::ReturnDon {
            dons: options.iter().copied().take(n).collect(),
        });
    }
    out
}

/// Walks every way of taking `n` DON!! across `groups`, emitting one action per
/// distribution. Larger takes from earlier groups come first.
fn distribute(
    groups: &[Vec<CardInstanceId>],
    n: usize,
    at: usize,
    taken: &mut Vec<CardInstanceId>,
    out: &mut Vec<Action>,
) {
    if out.len() >= MAX_CHOICE_SUBSETS {
        return;
    }
    if n == 0 {
        out.push(Action::ReturnDon {
            dons: taken.clone(),
        });
        return;
    }
    // Nothing left to take from, or not enough left to finish: prune rather
    // than walk a branch that cannot produce a legal answer.
    let remaining: usize = groups
        .get(at..)
        .unwrap_or_default()
        .iter()
        .map(Vec::len)
        .sum();
    if remaining < n {
        return;
    }
    let group = &groups[at];
    for k in (0..=group.len().min(n)).rev() {
        let before = taken.len();
        taken.extend(group.iter().copied().take(k));
        distribute(groups, n - k, at + 1, taken, out);
        taken.truncate(before);
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

/// Every legal answer to a `Choose`.
///
/// Split out of `legal_actions` so the floor and the truncation can be put
/// against each other without standing up a whole `Game` to do it.
fn choose_actions(options: &[CardInstanceId], up_to: u8, at_least: u8) -> Vec<Action> {
    // A mandatory choice with fewer legal cards than it asks for takes as many
    // as exist, rather than leaving the player no legal answer.
    let floor = (at_least as usize).min(options.len());
    let mut out: Vec<Action> = subsets(options, up_to as usize)
        .into_iter()
        .filter(|cards| cards.len() >= floor)
        .map(|cards| Action::Choose { cards })
        .collect();
    // `subsets` truncates by dropping the *largest* subsets, which are exactly
    // the ones a non-zero floor keeps — so a wide enough choice could filter
    // down to nothing and stall the game. Unreachable for any printed card, and
    // cheap to make impossible.
    if out.is_empty() {
        out.push(Action::Choose {
            cards: options.iter().copied().take(floor).collect(),
        });
    }
    out
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

    /// A mandatory choice must always leave a legal answer. `legal_actions`
    /// promises a non-empty list whenever something is pending, and the
    /// engine's own `advance` loop relies on it.
    #[test]
    fn a_mandatory_choice_always_has_a_legal_answer() {
        // Wide enough that `subsets` truncates before reaching any subset the
        // floor would keep, which is the case the fallback exists for.
        let options = ids(400);
        assert!(
            subsets(&options, 2).iter().all(|s| s.len() < 2),
            "precondition: truncation should leave nothing the floor accepts"
        );

        let out = choose_actions(&options, 2, 2);
        assert!(
            !out.is_empty(),
            "a mandatory choice with no legal answer stalls the game"
        );
        for action in &out {
            let Action::Choose { cards } = action else {
                panic!("expected a Choose, got {action:?}");
            };
            assert_eq!(cards.len(), 2, "the answer must satisfy the floor");
        }
    }

    /// The floor is what makes a choice mandatory: "trash 1 card from your
    /// hand" must not offer trashing none.
    #[test]
    fn a_floor_removes_the_answers_below_it() {
        let out = choose_actions(&ids(4), 2, 1);
        assert!(!out.is_empty());
        for action in &out {
            let Action::Choose { cards } = action else {
                panic!("expected a Choose, got {action:?}");
            };
            assert!(!cards.is_empty(), "a floor of 1 must not offer none");
        }
        // And without one, declining is still on the table (8-4-4-1).
        let offered = choose_actions(&ids(4), 2, 0);
        assert!(matches!(&offered[0], Action::Choose { cards } if cards.is_empty()));
    }
}
