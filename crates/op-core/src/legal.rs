//! Legal action enumeration.
//!
//! One function serves three consumers: the server validates incoming actions
//! against it, the RL env uses it as an action mask, and search agents use it
//! as the move generator. Keeping them on the same implementation is what stops
//! the three from disagreeing about what is legal.
//!
//! The one place this enumerates *up to equivalence* is `ReturnDon`, where
//! interchangeable DON!! would otherwise multiply one choice into hundreds of
//! id-sets. `Game::apply` still accepts any of them, so a consumer validating
//! by membership must canonicalise with `Game::don_class` first.

use crate::action::{Action, Pending};
use crate::card::Category;
use crate::derive;
use crate::game::{DonClass, Game};
use crate::ids::CardInstanceId;
use crate::zone::Zone;

/// Cap on enumerated subsets for a "choose up to N" request. Beyond this the
/// list is truncated rather than exploding; no printed card comes close.
const MAX_CHOICE_SUBSETS: usize = 256;

/// Cap on enumerated arrangements, which grow as `(n+1)!`.
///
/// Sized to cover the whole printed pool rather than reusing the subset cap:
/// the largest "look at N" in the corpus is 5, for 720 arrangements, and 256
/// would silently drop most of them. That matters more here than it does for a
/// choice, because a UI builds an arrangement and then looks for the matching
/// action — an arrangement that was truncated away is one the player can reach
/// and then cannot submit.
const MAX_ARRANGEMENTS: usize = 1024;

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
                        &derived,
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
        Pending::Arrange { cards, .. } => arrange_actions(cards),

        Pending::Choose {
            options,
            up_to,
            at_least,
            ..
        } => choose_actions(options, *up_to, *at_least),
    }
}

/// Every answer to a `ReturnDon` that differs from the others, one per
/// distribution over [`DonClass`] rather than per subset of ids — C(10,7) is
/// 120 id-sets and two or three real choices. Callers holding an id-set must
/// canonicalise it with `don_class` rather than compare ids to these.
fn return_don_actions(game: &Game, options: &[CardInstanceId], n: u8) -> Vec<Action> {
    let n = (n as usize).min(options.len());

    // Grouped in the pool's order, so the enumeration is deterministic.
    let mut classes: Vec<(DonClass, Vec<CardInstanceId>)> = Vec::new();
    for &don in options {
        let key = game.don_class(don);
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

/// Every way to send the looked-at cards back to the top or the bottom.
///
/// "In any order" is taken literally: the answer is two ordered lists, so a
/// player who has just seen three cards can decide which to bury *and* what
/// order the survivors sit in. Anything less would be a simplification, and
/// this one would be felt — unlike `DigTop`'s, the cards here are known.
///
/// There are `(n+1)!` arrangements of `n` cards, so this is 24 for the 3 that
/// ST03-010 looks at. It grows fast enough to need the same cap as `subsets`,
/// which no printed card comes near: the largest "look at" in the pool is 5,
/// for 720.
fn arrange_actions(cards: &[CardInstanceId]) -> Vec<Action> {
    let mut out = Vec::new();
    // Walk every ordering, then every split of that ordering into the part
    // going back on top and the part going to the bottom. Each arrangement is
    // produced once: the ordering fixes both lists, the split says where the
    // boundary falls.
    for order in permutations(cards) {
        for split in 0..=order.len() {
            out.push(Action::Arrange {
                top: order[..split].to_vec(),
                bottom: order[split..].to_vec(),
            });
            if out.len() >= MAX_ARRANGEMENTS {
                return out;
            }
        }
    }
    out
}

fn permutations(cards: &[CardInstanceId]) -> Vec<Vec<CardInstanceId>> {
    if cards.is_empty() {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for (i, &card) in cards.iter().enumerate() {
        let mut rest = cards.to_vec();
        rest.remove(i);
        for mut tail in permutations(&rest) {
            tail.insert(0, card);
            out.push(tail);
        }
    }
    out
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

    /// "In any order" means every ordered split, each offered once. Three cards
    /// give 3! orderings x 4 split points = 24, which is (n+1)!.
    ///
    /// The count is asserted alongside the uniqueness because either alone
    /// would miss half the bug: duplicates inflate the count, and a missing
    /// arrangement deflates it, and a set-based check sees neither.
    #[test]
    fn every_arrangement_is_offered_exactly_once() {
        let cards = ids(3);
        let out = arrange_actions(&cards);
        assert_eq!(out.len(), 24, "3! orderings x 4 splits");

        let mut seen: Vec<(Vec<CardInstanceId>, Vec<CardInstanceId>)> = out
            .iter()
            .map(|a| match a {
                Action::Arrange { top, bottom } => (top.clone(), bottom.clone()),
                other => panic!("expected Arrange, got {other:?}"),
            })
            .collect();
        let total = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), total, "no arrangement may be offered twice");

        // Both extremes are reachable: bury everything, or keep everything.
        assert!(seen.iter().any(|(t, _)| t.is_empty()));
        assert!(seen.iter().any(|(_, b)| b.is_empty()));
        // And every answer accounts for all three cards.
        assert!(seen.iter().all(|(t, b)| t.len() + b.len() == 3));
    }

    /// The largest "look at N" in the printed pool is 5, so the cap has to
    /// clear 6! = 720. A UI builds an arrangement and then looks for the action
    /// that matches it, so one truncated away is one the player can reach and
    /// then cannot submit — a dead end rather than a missing option.
    #[test]
    fn the_largest_printed_look_is_enumerated_in_full() {
        // 720 is 6!, and it arriving in full is the assertion: the cap would
        // show up here as a shorter list.
        assert_eq!(arrange_actions(&ids(5)).len(), 720);
    }

    /// The looked-at cards are never partly placed: an arrangement names all of
    /// them or the engine rejects it, so nothing can be stranded in limbo.
    #[test]
    fn an_arrangement_covers_every_card_it_was_given() {
        for n in 0..=4u32 {
            let cards = ids(n);
            for action in arrange_actions(&cards) {
                let Action::Arrange { top, bottom } = action else {
                    unreachable!()
                };
                let mut named: Vec<_> = top.iter().chain(bottom.iter()).copied().collect();
                named.sort();
                let mut expected = cards.clone();
                expected.sort();
                assert_eq!(named, expected, "every card placed exactly once");
            }
        }
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

    /// A floor above 1 removes every smaller subset, singletons included.
    ///
    /// Worth pinning because reading a candidate list off the single-card
    /// answers is sound only while the floor is 0 or 1, and ST03-005 ("draw 2
    /// cards and trash 2 cards from your hand") is the first card above it.
    /// Every candidate is still present in the union of the answers, which is
    /// what a consumer has to read instead.
    #[test]
    fn a_floor_above_one_leaves_no_single_card_answers() {
        let cards = ids(4);
        let out = choose_actions(&cards, 2, 2);
        assert!(!out.is_empty());

        let mut union: Vec<CardInstanceId> = Vec::new();
        for action in &out {
            let Action::Choose { cards } = action else {
                panic!("expected a Choose, got {action:?}")
            };
            assert_eq!(cards.len(), 2, "a floor of 2 admits no smaller answer");
            for &id in cards {
                if !union.contains(&id) {
                    union.push(id);
                }
            }
        }
        union.sort();
        assert_eq!(
            union, cards,
            "every candidate is reachable through some answer"
        );
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
