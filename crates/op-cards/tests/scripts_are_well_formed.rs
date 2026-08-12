//! Static checks over every script this crate ships.
//!
//! Deliberately independent of `data/`: these tests are the one card-level
//! guard that still runs on a bare clone. The exception is
//! [`every_on_block_script_is_on_a_blocker`], which asks a question about
//! printed keywords and so has to skip itself without the data.

mod common;

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
                        from: None,
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
/// needs. The four cards below are the whole of it today; a new one that
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
        // ST08-001 joined when ST-08 was scripted: "[Your Turn] When a
        // Character is K.O.'d, give up to 1 rested DON!! card to this Leader."
        // Same printed phrasing as the ST-01 three, so same source.
        vec!["ST01-001", "ST01-007", "ST01-011", "ST08-001"],
        "the set of DON!!-giving cards changed; check each against its printed text"
    );
}

/// An `[On Block]` effect fires from `resolve_block`, which only ever runs for a
/// card `legal_blockers` returned — and that requires the printed `[Blocker]`
/// keyword. A script carrying `[On Block]` text on a card without it is dead,
/// and `validate_script` cannot see the omission because it never reads a
/// `CardDef`.
///
/// This is the guard `Timing::is_activated_by_engine` used to provide, moved to
/// the only place that can still ask the question.
#[test]
fn every_on_block_script_is_on_a_blocker() {
    use op_core::card::Keyword;
    use op_core::effect::Timing;

    let Some(db) = common::card_db() else {
        eprintln!("skipping: run the ingest to populate data/");
        return;
    };

    for (number, script) in all_scripts() {
        if !script.auto.iter().any(|a| a.timing == Timing::OnBlock) {
            continue;
        }
        let Some(def) = db.by_number(number) else {
            continue;
        };
        assert!(
            db.get(def).keywords.contains(&Keyword::Blocker),
            "{number} has an [On Block] effect but no printed [Blocker], so it \
             can never be the card that blocks and the effect can never fire"
        );
    }
}

/// A card with printed `[Trigger]` text must script it.
///
/// Nothing else asks. `coverage` counts a card done the moment its script is
/// not vanilla, so a card whose body is scripted and whose `[Trigger]` is not
/// reports `OK` and earns its set a place in the badge; `KEYWORD_ONLY` asserts
/// the opposite outright, that the card needs no script at all. Four ST-03
/// cards shipped without their `[Trigger]` behind exactly those two blind
/// spots, one of them a `[Blocker]` sitting in `KEYWORD_ONLY`.
#[test]
fn every_printed_trigger_is_scripted() {
    let Some(db) = common::card_db() else {
        eprintln!("skipping: run the ingest to populate data/");
        return;
    };

    let scripted: std::collections::BTreeMap<&str, &CardScript> = all_scripts_map();
    let mut missing: Vec<String> = Vec::new();

    for (def, card) in db.iter() {
        if card.trigger.is_none() {
            continue;
        }
        // Only sets this crate ships. An unscripted set is not an omission.
        let Some(script) = scripted.get(card.number.as_str()) else {
            if op_cards::KEYWORD_ONLY.contains(&card.number.as_str()) {
                missing.push(format!(
                    "{} is KEYWORD_ONLY but has printed [Trigger] text",
                    card.number
                ));
            }
            let _ = def;
            continue;
        };
        if script.trigger.is_empty() {
            missing.push(card.number.clone());
        }
    }
    missing.sort();

    // Pinned rather than allowed: the assertion is equality, so a new omission
    // fails it and so does fixing one of these without removing it from the
    // list. ST-06 shipped these three before anything asked the question, and
    // one of them — ST06-015, "your opponent chooses 1 card from their hand and
    // trashes it" — needs a decision aimed at the player who is not the
    // effect's controller, which `Pending::Choose` cannot express today.
    let known: Vec<String> = ["ST06-014", "ST06-015", "ST06-016"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        missing, known,
        "printed [Trigger] text with no script; update the pinned list only \
         when the cards themselves change"
    );
}

fn all_scripts_map() -> std::collections::BTreeMap<&'static str, &'static CardScript> {
    // Leaked once so the map can borrow: this is a test binary and the scripts
    // live for the whole run either way.
    let scripts: &'static [(&'static str, CardScript)] =
        Box::leak(all_scripts().into_boxed_slice());
    scripts.iter().map(|(n, s)| (*n, s)).collect()
}
