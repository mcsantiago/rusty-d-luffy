//! Prints the card packs the test suite needs, space-separated.
//!
//! CI fetches card data for a list of packs, and that list used to be typed
//! into `ci.yml`. It was the sixth copy of "which sets do we care about", and
//! it drifted the moment ST-04 and ST-08 were scripted: the card-level tests
//! fetched no ST-04 data and failed with `UnknownCard("ST04-001")`.
//!
//! Deriving it from the built-in decks means a set is fetched because it is
//! playable, which is the actual reason CI wants it:
//!
//! ```sh
//! cargo run -q -p op-cards --bin packs
//! # ST-01 ST-02 ST-04 ST-06 ST-08
//! ```
//!
//! The hyphen is what `op-fetch` expects (`ST-01`), while a deck id has none
//! (`ST01`), so the conversion lives here rather than in either caller.

fn main() {
    let packs: Vec<String> = op_cards::decks::ALL
        .iter()
        .map(|d| format!("{}-{}", &d.id[..2], &d.id[2..]))
        .collect();
    println!("{}", packs.join(" "));
}
