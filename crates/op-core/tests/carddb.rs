//! Checks the kernel against the real ingested card data.
//!
//! Skipped when `data/` has not been populated, so the suite still runs on a
//! fresh clone. Populate it with `python3 tools/ingest/fetch_cards.py`.

mod common;

use op_core::card::{CardDb, CardDef, Category, Color, Keyword};
use op_core::view::PlayerView;
use op_core::PlayerId;

fn data_dir() -> Option<std::path::PathBuf> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/cards")
        .canonicalize()
        .ok()?;
    dir.exists().then_some(dir)
}

/// Alternate printings share their base card's *card number*, which is what the
/// four-copy limit counts against (5-1-2-3). If `OP01-016_p1` were loaded as its
/// own def, a deck could hold four of each and field eight — so the loader must
/// never produce a number containing a variant suffix.
#[test]
fn alternate_printings_never_become_separate_cards() {
    let Some(dir) = data_dir() else {
        eprintln!("skipping: run tools/ingest/fetch_cards.py to populate data/");
        return;
    };
    let db = CardDb::load_dir(dir).expect("ingested card data should load");

    for (_, def) in db.iter() {
        assert!(
            !def.number.contains('_'),
            "{} is an alternate printing and should have been dropped at load",
            def.number
        );
    }
}

/// Guards the same property without needing `data/`: writing a parallel art
/// into a directory must not add a second def.
#[test]
fn a_parallel_art_file_does_not_add_a_second_def() {
    let dir = std::env::temp_dir().join(format!("opsim-parallel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");

    let card = |id: &str| {
        format!(
            r#"{{"id":"{id}","name":"Nami","category":"Character","colors":["Red"],
                 "cost":1,"power":1000,"counter":1000,"types":["Straw Hat Crew"],
                 "attributes":[],"effect":null,"trigger":null,"pack_id":"569001"}}"#
        )
    };
    std::fs::write(dir.join("ST01-007.json"), card("ST01-007")).unwrap();
    std::fs::write(dir.join("ST01-007_p1.json"), card("ST01-007_p1")).unwrap();
    std::fs::write(dir.join("ST01-007_r1.json"), card("ST01-007_r1")).unwrap();

    let db = CardDb::load_dir(&dir).expect("should load");
    std::fs::remove_dir_all(&dir).ok();

    // The DON!! def plus exactly one Nami.
    assert_eq!(db.len(), 2, "expected only DON!! and one ST01-007");
    assert!(db.by_number("ST01-007").is_some());
    assert!(db.by_number("ST01-007_p1").is_none());
    assert!(db.by_number("ST01-007_r1").is_none());

    // Sanity: the loader is not simply rejecting everything with a suffix-like
    // shape — a hand-inserted def is unaffected.
    let mut db = db;
    db.insert(CardDef {
        number: "P-001".into(),
        name: "Promo".into(),
        category: Category::Character,
        colors: vec![Color::Red],
        cost: 1,
        life: None,
        power: Some(1000),
        counter: None,
        types: vec![],
        attributes: vec![],
        keywords: vec![],
        effect: None,
        trigger: None,
    });
    assert!(db.by_number("P-001").is_some());
}

/// Loads `cards` as a one-file pack, the way the ingest writes them.
fn db_from(tag: &str, cards: &[String]) -> CardDb {
    let dir = std::env::temp_dir().join(format!("opsim-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("pack.json"), format!("[{}]", cards.join(","))).unwrap();
    let db = CardDb::load_dir(&dir).expect("should load");
    std::fs::remove_dir_all(&dir).ok();
    db
}

/// One card as punk-records writes it. `cost` doubles as a Leader's Life.
fn raw_card(id: &str, category: &str, cost: u8, power: &str, counter: &str) -> String {
    format!(
        r#"{{"id":"{id}","name":"{id}","category":"{category}","colors":["Red"],
             "cost":{cost},"power":{power},"counter":{counter},"types":["Test"],
             "attributes":[],"effect":null,"trigger":null}}"#
    )
}

/// 2-6-2 gives every Character a power, but upstream writes `"power": null`
/// for the 149 whose printed value is 0 (ST08-009 Makino, OP01-006 Otama).
/// Both clients key their power badge off `Option::is_some`, so a null there
/// makes a 0-power Character read as "power unknown" rather than "power 0".
#[test]
fn a_character_with_null_printed_power_has_power_zero() {
    let db = db_from(
        "zero-power",
        &[
            raw_card("LDR-P", "Leader", 5, "5000", "null"),
            raw_card("CHR-ZERO", "Character", 1, "null", "2000"),
        ],
    );

    let zero = db.get(db.by_number("CHR-ZERO").expect("CHR-ZERO present"));
    assert_eq!(zero.power, Some(0), "a printed power of 0 is not no power");

    // And it survives the trip to a client: the hand is projected with the
    // power the clients draw, and `None` is what they render as nothing.
    let cards = common::TestCards { db };
    let (game, _) = common::game_with(
        &cards,
        common::TestScripts::default(),
        1,
        ("LDR-P", common::deck_of("CHR-ZERO", 50)),
        ("LDR-P", common::deck_of("CHR-ZERO", 50)),
    );
    let derived = game.derived();
    let view = PlayerView::project(&game.state, game.db(), &derived, PlayerId::P0);
    for card in &view.you.hand {
        assert_eq!(card.power, Some(0), "a 0-power Character must report its 0");
    }
    assert!(!view.you.hand.is_empty(), "setup should deal a hand");
}

/// The other half of 2-6-2: Events and Stages have no power, and must keep
/// reporting none however upstream happens to spell the field.
#[test]
fn events_and_stages_have_no_power() {
    let db = db_from(
        "no-power",
        &[
            raw_card("EVT-001", "Event", 1, "null", "null"),
            raw_card("STG-001", "Stage", 1, "null", "null"),
            // Upstream has never printed a power on either, but the category is
            // what decides, so a stray number must not create one.
            raw_card("EVT-002", "Event", 1, "1000", "null"),
            raw_card("STG-002", "Stage", 1, "0", "null"),
        ],
    );

    for number in ["EVT-001", "STG-001", "EVT-002", "STG-002"] {
        let def = db.get(db.by_number(number).expect("card present"));
        assert_eq!(
            def.power, None,
            "{number} is a {:?} and has no power",
            def.category
        );
    }
}

/// The same two halves against the real pool, which is where the 149 live.
#[test]
fn every_ingested_character_has_a_power() {
    let Some(dir) = data_dir() else {
        eprintln!("skipping: run tools/ingest/fetch_cards.py to populate data/");
        return;
    };
    let db = CardDb::load_dir(dir).expect("ingested card data should load");

    for (_, def) in db.iter() {
        match def.category {
            Category::Leader | Category::Character => assert!(
                def.power.is_some(),
                "{} is a {:?} with no power",
                def.number,
                def.category
            ),
            _ => assert_eq!(def.power, None, "{} should have no power", def.number),
        }
    }

    // ST08-009 Makino prints "Power: 0" and upstream writes it null. A partial
    // fetch (`op-fetch --packs`) may not hold ST-08, so say so rather than
    // passing quietly on a pool that cannot answer.
    let Some(makino) = db.by_number("ST08-009") else {
        eprintln!("ST-08 not in data/: skipping the printed-zero case");
        return;
    };
    assert_eq!(
        db.get(makino).power,
        Some(0),
        "ST08-009 Makino prints Power: 0"
    );
}

#[test]
fn ingested_starter_decks_load_and_parse() {
    let Some(dir) = data_dir() else {
        eprintln!("skipping: run tools/ingest/fetch_cards.py to populate data/");
        return;
    };
    let db = CardDb::load_dir(dir).expect("ingested card data should load");

    // ST-01 and ST-02 are 17 cards each, plus the synthetic DON!!.
    assert!(
        db.len() >= 34,
        "expected at least 34 cards, got {}",
        db.len()
    );

    // ST01-001 Monkey.D.Luffy: a Leader with 5 Life and 5000 power. Upstream
    // stores Life in the cost field, so this pins that mapping against real
    // data rather than only against the unit-test fixture.
    let luffy = db.get(db.by_number("ST01-001").expect("ST01-001 present"));
    assert_eq!(luffy.category, Category::Leader);
    assert_eq!(luffy.life, Some(5));
    assert_eq!(luffy.power, Some(5000));
    assert!(luffy.has_type("Straw Hat Crew"));

    // ST01-006 Tony Tony.Chopper prints [Blocker] unconditionally.
    let chopper = db.get(db.by_number("ST01-006").expect("ST01-006 present"));
    assert!(chopper.has_keyword(Keyword::Blocker));
    assert_eq!(chopper.counter, None);

    // ST01-004 Sanji only *gains* [Rush] with [DON!! x2], so it must not be
    // treated as a printed keyword.
    let sanji = db.get(db.by_number("ST01-004").expect("ST01-004 present"));
    assert!(!sanji.has_keyword(Keyword::Rush));

    // ST01-012 Luffy prints [Rush] outright.
    let rush_luffy = db.get(db.by_number("ST01-012").expect("ST01-012 present"));
    assert!(rush_luffy.has_keyword(Keyword::Rush));

    // ST01-014 Guard Point is a Counter Event with a [Trigger].
    let guard = db.get(db.by_number("ST01-014").expect("ST01-014 present"));
    assert_eq!(guard.category, Category::Event);
    assert!(guard.trigger.is_some());

    // Every Character with a Counter value parsed it as a number.
    for (_, def) in db.iter() {
        if def.category == Category::Character {
            if let Some(counter) = def.counter {
                assert!(
                    (0..=5000).contains(&counter),
                    "{} has an implausible Counter value {counter}",
                    def.number
                );
            }
        }
        if def.category == Category::Leader {
            assert!(
                def.life.is_some(),
                "{} is a Leader with no Life",
                def.number
            );
        }
    }
}
