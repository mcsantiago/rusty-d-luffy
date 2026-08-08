//! Static checks on card scripts.
//!
//! Scripts are data. The kernel executes them defensively — an op that reads a
//! binding nobody bound gets an empty slice and does nothing (see
//! [`EffectFrame::bound`]) — so a typo does not crash, it produces a card that
//! silently has no text. Sakazuki appearing to do nothing cost a trace review
//! before this module existed.
//!
//! Today the Rust builders in `op-cards` catch most mistakes at compile time,
//! but binding keys are strings and timings are free-form, and neither is
//! checked by the type system. The checks here close that gap, and are the
//! precondition for ever loading scripts from a file: external data with no
//! validation fails invisibly.
//!
//! [`EffectFrame::bound`]: crate::effect::EffectFrame::bound

use std::collections::BTreeSet;
use std::fmt;

use crate::effect::{Condition, EffectOp, Timing, SELF_BINDING};
use crate::script::{ActivationCost, CardScript, TARGET_BINDING};

/// Which part of a script a [`Diagnostic`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    Permanent(usize),
    Auto { index: usize, timing: Timing },
    Activated(usize),
    Counter,
    Trigger,
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Site::Permanent(i) => write!(f, "permanent[{i}]"),
            Site::Auto { index, timing } => write!(f, "auto[{index}] {timing:?}"),
            Site::Activated(i) => write!(f, "activated[{i}]"),
            Site::Counter => write!(f, "counter"),
            Site::Trigger => write!(f, "trigger"),
        }
    }
}

/// What is wrong. Every variant describes a script that compiles, runs, and
/// does less than its printed text says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// An op reads a key nothing ever binds — usually a typo on one side of the
    /// pair.
    UnboundKey { key: String, op: &'static str },
    /// An op reads a key that *is* bound, but later in the same op list.
    /// Bindings are filled in as the ops run, so the read sees nothing.
    ReadBeforeBound { key: String, op: &'static str },
    /// [`TARGET_BINDING`] read where the engine does not supply it. Only
    /// `[Counter]` and `[Trigger]` effects get a pre-bound target.
    TargetNotSupplied { op: &'static str },
    /// A `Choose`/`DigTop` whose result nothing consumes: the player is asked a
    /// question that changes nothing.
    UnreadBinding { key: String },
    /// A `Choose`/`DigTop` on a key the engine pre-binds. `Choose` skips itself
    /// when the key already has a binding, so the op never runs — and every
    /// read of that key sees the engine's value instead.
    ShadowsSuppliedBinding { key: String },
    /// A timing the engine never activates. The whole effect is dead.
    UnreachableTiming { timing: Timing },
    /// The cost rests the source, which requires it active, while a condition
    /// requires it rested (8-4-1-1 checks conditions before the cost is paid).
    /// The effect can never be activated.
    RestSelfRequiresActive,
    /// `[Your Turn]` and `[Opponent's Turn]` on the same effect (8-3-2-1
    /// requires all conditions to hold).
    ContradictoryTurnConditions,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::UnboundKey { key, op } => {
                write!(f, "{op} reads binding {key:?}, which nothing binds")
            }
            Problem::ReadBeforeBound { key, op } => write!(
                f,
                "{op} reads binding {key:?} before the op that binds it, so it always sees nothing"
            ),
            Problem::TargetNotSupplied { op } => write!(
                f,
                "{op} reads the engine-supplied {TARGET_BINDING:?} binding, \
                 which only [Counter] and [Trigger] effects are given"
            ),
            Problem::UnreadBinding { key } => {
                write!(f, "binding {key:?} is chosen but never read")
            }
            Problem::ShadowsSuppliedBinding { key } => write!(
                f,
                "binds {key:?}, which the engine already supplies; \
                 an op that binds an already-bound key is skipped, so this never runs"
            ),
            Problem::UnreachableTiming { timing } => {
                write!(f, "timing {timing:?} is never activated by the engine")
            }
            Problem::RestSelfRequiresActive => write!(
                f,
                "the cost rests this card, which requires it to be active, \
                 but a condition requires it to be rested already"
            ),
            Problem::ContradictoryTurnConditions => {
                write!(f, "requires both [Your Turn] and [Opponent's Turn]")
            }
        }
    }
}

/// One problem, located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub site: Site,
    pub problem: Problem,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.site, self.problem)
    }
}

/// Checks one script. An empty result means the script is well-formed; it does
/// *not* mean the script matches its printed text.
///
/// Diagnostics come out in a fixed order — script order, then key order — so
/// callers can compare runs.
pub fn validate_script(script: &CardScript) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    for (i, effect) in script.permanent.iter().enumerate() {
        check_conditions(
            Site::Permanent(i),
            &effect.conditions,
            &ActivationCost::default(),
            &mut out,
        );
    }

    for (index, effect) in script.auto.iter().enumerate() {
        let site = Site::Auto {
            index,
            timing: effect.timing,
        };
        if !effect.timing.is_activated_by_engine() {
            out.push(Diagnostic {
                site,
                problem: Problem::UnreachableTiming {
                    timing: effect.timing,
                },
            });
        }
        check_conditions(site, &effect.conditions, &effect.cost, &mut out);
        check_ops(site, &effect.ops, &[SELF_BINDING], &mut out);
    }

    for (i, effect) in script.activated.iter().enumerate() {
        let site = Site::Activated(i);
        check_conditions(site, &effect.conditions, &effect.cost, &mut out);
        check_ops(site, &effect.ops, &[SELF_BINDING], &mut out);
    }

    // 10-2-4: the defender picks the card a [Counter] boosts before the Event
    // resolves, and 10-1-5-3 hands a [Trigger] its own card the same way.
    let supplied = [SELF_BINDING, TARGET_BINDING];
    check_ops(Site::Counter, &script.counter, &supplied, &mut out);
    check_ops(Site::Trigger, &script.trigger, &supplied, &mut out);

    out
}

/// Costs and conditions that cannot both be satisfied, whatever the game state.
fn check_conditions(
    site: Site,
    conditions: &[Condition],
    cost: &ActivationCost,
    out: &mut Vec<Diagnostic>,
) {
    if cost.rest_self && conditions.contains(&Condition::SelfRested) {
        out.push(Diagnostic {
            site,
            problem: Problem::RestSelfRequiresActive,
        });
    }
    if conditions.contains(&Condition::YourTurn) && conditions.contains(&Condition::OpponentsTurn) {
        out.push(Diagnostic {
            site,
            problem: Problem::ContradictoryTurnConditions,
        });
    }
}

/// Walks an op list in execution order, tracking which keys are bound by the
/// time each op runs.
fn check_ops(site: Site, ops: &[EffectOp], supplied: &[&str], out: &mut Vec<Diagnostic>) {
    let bound_eventually: BTreeSet<&str> = ops.iter().filter_map(EffectOp::binds).collect();
    let mut bound: BTreeSet<&str> = supplied.iter().copied().collect();
    let mut read: BTreeSet<&str> = BTreeSet::new();

    for op in ops {
        if let Some(key) = op.reads() {
            read.insert(key);
            if !bound.contains(key) {
                let op = op.name();
                let key = key.to_string();
                out.push(Diagnostic {
                    site,
                    problem: if bound_eventually.contains(key.as_str()) {
                        Problem::ReadBeforeBound { key, op }
                    } else if key == TARGET_BINDING {
                        Problem::TargetNotSupplied { op }
                    } else {
                        Problem::UnboundKey { key, op }
                    },
                });
            }
        }
        if let Some(key) = op.binds() {
            // `DigTop` is its own consumer: it suspends on the choice, then
            // re-runs and reads the binding back to add the chosen cards to
            // hand and bottom the rest (ST02-007). No other op has to read it.
            if matches!(op, EffectOp::DigTop { .. }) {
                read.insert(key);
            }
            if supplied.contains(&key) {
                out.push(Diagnostic {
                    site,
                    problem: Problem::ShadowsSuppliedBinding {
                        key: key.to_string(),
                    },
                });
            }
            bound.insert(key);
        }
    }

    for key in bound_eventually.difference(&read) {
        out.push(Diagnostic {
            site,
            problem: Problem::UnreadBinding {
                key: key.to_string(),
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::Category;
    use crate::effect::{Duration, Filter, ModKind, Selector, Who};
    use crate::script::{ActivatedEffect, AutoEffect, PermanentEffect, Scope};
    use crate::zone::Zone;

    fn pick(key: &str) -> EffectOp {
        EffectOp::Choose {
            key: key.to_string(),
            select: Selector {
                zone: Zone::Character,
                owner: Who::Opponent,
                up_to: 1,
                filters: vec![Filter::IsCategory(Category::Character)],
            },
        }
    }

    fn ko(key: &str) -> EffectOp {
        EffectOp::Ko {
            key: key.to_string(),
        }
    }

    fn auto(timing: Timing, ops: Vec<EffectOp>) -> AutoEffect {
        AutoEffect {
            timing,
            conditions: Vec::new(),
            cost: ActivationCost::default(),
            ops,
            slot: 0,
            once_per_turn: false,
        }
    }

    fn problems(script: &CardScript) -> Vec<Problem> {
        validate_script(script)
            .into_iter()
            .map(|d| d.problem)
            .collect()
    }

    #[test]
    fn a_well_formed_script_reports_nothing() {
        let script = CardScript {
            auto: vec![auto(Timing::OnPlay, vec![pick("t"), ko("t")])],
            ..CardScript::default()
        };
        assert_eq!(problems(&script), []);
    }

    #[test]
    fn the_self_binding_needs_no_choose() {
        let script = CardScript {
            auto: vec![auto(
                Timing::EndOfYourTurn,
                vec![EffectOp::SetActive {
                    key: SELF_BINDING.to_string(),
                }],
            )],
            ..CardScript::default()
        };
        assert_eq!(problems(&script), []);
    }

    /// The Sakazuki failure: a `Choose` bound to "t" read back as "target".
    #[test]
    fn a_key_nothing_binds_is_reported_on_both_sides() {
        let script = CardScript {
            auto: vec![auto(Timing::OnPlay, vec![pick("t"), ko("target")])],
            ..CardScript::default()
        };
        assert_eq!(
            problems(&script),
            [
                Problem::TargetNotSupplied { op: "Ko" },
                Problem::UnreadBinding {
                    key: "t".to_string()
                },
            ]
        );
    }

    #[test]
    fn a_misspelled_key_is_unbound() {
        let script = CardScript {
            activated: vec![ActivatedEffect {
                conditions: Vec::new(),
                cost: ActivationCost::default(),
                ops: vec![pick("chosen"), ko("chosn")],
                slot: 0,
                once_per_turn: false,
            }],
            ..CardScript::default()
        };
        assert_eq!(
            problems(&script),
            [
                Problem::UnboundKey {
                    key: "chosn".to_string(),
                    op: "Ko"
                },
                Problem::UnreadBinding {
                    key: "chosen".to_string()
                },
            ]
        );
    }

    #[test]
    fn reading_a_key_before_its_choose_is_reported_once() {
        let script = CardScript {
            auto: vec![auto(Timing::OnPlay, vec![ko("t"), pick("t")])],
            ..CardScript::default()
        };
        assert_eq!(
            problems(&script),
            [Problem::ReadBeforeBound {
                key: "t".to_string(),
                op: "Ko"
            }]
        );
    }

    #[test]
    fn target_is_supplied_to_counter_and_trigger_only() {
        let boost = EffectOp::Modify {
            key: TARGET_BINDING.to_string(),
            kind: ModKind::Power(2000),
            duration: Duration::ThisBattle,
        };
        let ok = CardScript {
            counter: vec![boost.clone()],
            trigger: vec![boost.clone()],
            ..CardScript::default()
        };
        assert_eq!(problems(&ok), []);

        let bad = CardScript {
            auto: vec![auto(Timing::OnPlay, vec![boost])],
            ..CardScript::default()
        };
        assert_eq!(
            problems(&bad),
            [Problem::TargetNotSupplied { op: "Modify" }]
        );
    }

    #[test]
    fn choosing_into_an_engine_supplied_key_never_runs() {
        let script = CardScript {
            counter: vec![
                pick(TARGET_BINDING),
                EffectOp::Rest {
                    key: TARGET_BINDING.to_string(),
                },
            ],
            ..CardScript::default()
        };
        assert_eq!(
            problems(&script),
            [Problem::ShadowsSuppliedBinding {
                key: TARGET_BINDING.to_string()
            }]
        );
    }

    /// Unlike `Choose`, a lone `DigTop` is complete on its own — it re-runs
    /// after the choice and consumes its own binding (ST02-007).
    #[test]
    fn a_dig_needs_no_other_reader() {
        let script = CardScript {
            activated: vec![ActivatedEffect {
                conditions: Vec::new(),
                cost: ActivationCost::default(),
                ops: vec![EffectOp::DigTop {
                    n: 5,
                    key: "found".to_string(),
                    up_to: 1,
                    filters: Vec::new(),
                }],
                slot: 0,
                once_per_turn: false,
            }],
            ..CardScript::default()
        };
        assert_eq!(problems(&script), []);
    }

    #[test]
    fn a_choose_that_nothing_reads_is_a_dead_binding() {
        let script = CardScript {
            trigger: vec![pick("found")],
            ..CardScript::default()
        };
        assert_eq!(
            problems(&script),
            [Problem::UnreadBinding {
                key: "found".to_string()
            }]
        );
    }

    #[test]
    fn timings_the_engine_never_fires_are_rejected() {
        for timing in [Timing::OnKo, Timing::OnBlock, Timing::Trigger] {
            let script = CardScript {
                auto: vec![auto(
                    timing,
                    vec![EffectOp::Draw {
                        player: Who::You,
                        n: 1,
                    }],
                )],
                ..CardScript::default()
            };
            assert_eq!(problems(&script), [Problem::UnreachableTiming { timing }]);
        }
    }

    #[test]
    fn resting_the_source_cannot_pay_for_an_effect_that_needs_it_rested() {
        let script = CardScript {
            activated: vec![ActivatedEffect {
                conditions: vec![Condition::SelfRested],
                cost: ActivationCost {
                    rest_don: 0,
                    rest_self: true,
                    trash_from_hand: 0,
                },
                ops: vec![EffectOp::Draw {
                    player: Who::You,
                    n: 1,
                }],
                slot: 0,
                once_per_turn: false,
            }],
            ..CardScript::default()
        };
        assert_eq!(problems(&script), [Problem::RestSelfRequiresActive]);
    }

    #[test]
    fn an_effect_cannot_want_both_turns() {
        let script = CardScript {
            permanent: vec![PermanentEffect {
                conditions: vec![Condition::YourTurn, Condition::OpponentsTurn],
                scope: Scope::ThisCard,
                kind: ModKind::Power(1000),
            }],
            ..CardScript::default()
        };
        assert_eq!(problems(&script), [Problem::ContradictoryTurnConditions]);
    }

    #[test]
    fn diagnostics_name_the_effect_they_came_from() {
        let script = CardScript {
            auto: vec![
                auto(Timing::OnPlay, vec![pick("a"), ko("a")]),
                auto(Timing::WhenAttacking, vec![ko("b")]),
            ],
            ..CardScript::default()
        };
        let found = validate_script(&script);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].to_string(),
            "auto[1] WhenAttacking: Ko reads binding \"b\", which nothing binds"
        );
    }
}
