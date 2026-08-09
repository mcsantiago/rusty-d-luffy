//! Static checks over every script this crate ships.
//!
//! Deliberately independent of `data/`: these tests are the one card-level
//! guard that still runs on a bare clone.

use op_cards::{all_scripts, validate_all_scripts};
use op_core::script::CardScript;

#[test]
fn every_script_is_well_formed() {
    let problems = validate_all_scripts();
    assert!(
        problems.is_empty(),
        "{} script problem(s):\n{}",
        problems.len(),
        problems
            .iter()
            .map(|(number, d)| format!("  {number} {d}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// The validator is only worth anything if it fails on a broken script. This
/// pins the case that motivated it: a `Choose` bound to one key read back under
/// another, which the engine executes without complaint as a no-op.
#[test]
fn the_validator_rejects_a_script_with_a_mismatched_key() {
    use op_core::effect::{EffectOp, Selector, Timing, Who};
    use op_core::script::{ActivationCost, AutoEffect};
    use op_core::validate::validate_script;
    use op_core::zone::Zone;

    let script = CardScript {
        auto: vec![AutoEffect {
            timing: Timing::OnPlay,
            conditions: Vec::new(),
            cost: ActivationCost::default(),
            ops: vec![
                EffectOp::Choose {
                    key: "t".to_string(),
                    select: Selector {
                        zone: Zone::Character,
                        owner: Who::Opponent,
                        up_to: 1,
                        at_least: 0,
                        filters: Vec::new(),
                    },
                },
                EffectOp::Ko {
                    key: "target".to_string(),
                },
            ],
            slot: 0,
            once_per_turn: false,
        }],
        ..CardScript::default()
    };
    assert!(!validate_script(&script).is_empty());
}

/// The JSON dump is only useful if it is a faithful representation. Scripts are
/// serialised as a CI artifact today and are the candidate format for runtime
/// loading, so a type that serialises but does not come back is a trap.
#[test]
fn every_script_survives_a_json_round_trip() {
    for (number, script) in all_scripts() {
        let json = serde_json::to_string(&script).expect("serialise");
        let back: CardScript = serde_json::from_str(&json)
            .unwrap_or_else(|err| panic!("{number} does not deserialise: {err}"));
        assert_eq!(script, back, "{number} changed across a round trip");
    }
}

/// Every card whose text reads "rested DON!! card" must select a rested one.
///
/// Bandai's ST01-001 ruling settles that the adjective qualifies the DON!!
/// being selected rather than the state it ends up in, so a script that asked
/// for `Any` here would let the effect spend an active DON!! the player still
/// needs. The three cards below are the whole of it today; a new one that
/// genuinely takes an active DON!! by effect should be added to the expected
/// set deliberately, not by relaxing the assertion.
#[test]
fn a_rested_don_effect_selects_only_rested_don() {
    use op_core::effect::{DonSource, EffectOp};

    let mut seen: Vec<&str> = Vec::new();
    for (number, script) in all_scripts() {
        let ops = script
            .activated
            .iter()
            .flat_map(|e| e.ops.iter())
            .chain(script.auto.iter().flat_map(|e| e.ops.iter()))
            .chain(script.trigger.iter());
        for op in ops {
            if let EffectOp::GiveDon { source, .. } = op {
                assert_eq!(
                    *source,
                    DonSource::Rested,
                    "{number} gives DON!! from {source:?}; every printed give today reads \
                     \"rested DON!! card\""
                );
                seen.push(number);
            }
        }
    }

    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        seen,
        vec!["ST01-001", "ST01-007", "ST01-011"],
        "the set of DON!!-giving cards changed; check each against its printed text"
    );
}
