//! Checks the kernel against the real ingested card data.
//!
//! Skipped when `data/` has not been populated, so the suite still runs on a
//! fresh clone. Populate it with `python3 tools/ingest/fetch_cards.py`.

use op_core::card::{CardDb, Category, Keyword};

fn data_dir() -> Option<std::path::PathBuf> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/cards")
        .canonicalize()
        .ok()?;
    dir.exists().then_some(dir)
}

#[test]
fn ingested_starter_decks_load_and_parse() {
    let Some(dir) = data_dir() else {
        eprintln!("skipping: run tools/ingest/fetch_cards.py to populate data/");
        return;
    };
    let db = CardDb::load_dir(dir).expect("ingested card data should load");

    // ST-01 and ST-02 are 17 cards each, plus the synthetic DON!!.
    assert!(db.len() >= 34, "expected at least 34 cards, got {}", db.len());

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
            assert!(def.life.is_some(), "{} is a Leader with no Life", def.number);
        }
    }
}
