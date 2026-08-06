//! Checks the kernel against the real ingested card data.
//!
//! Skipped when `data/` has not been populated, so the suite still runs on a
//! fresh clone. Populate it with `python3 tools/ingest/fetch_cards.py`.

use op_core::card::{CardDb, CardDef, Category, Color, Keyword};

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
