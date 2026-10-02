//! Startup cost and per-lookup cost of the embedded models.
//!
//!   cargo run --release --example bench

use std::time::Instant;
use typesuggest::dict::{Context, Dictionary};

fn main() {
    let t = Instant::now();
    let mut dict = Dictionary::from_frequency_text(include_str!("../data/en_50k.txt"));
    let after_words = t.elapsed();
    dict.load_bigrams_tsv(include_str!("../data/bigrams.tsv"));
    let after_bigrams = t.elapsed();
    dict.load_trigrams_tsv(include_str!("../data/trigrams.tsv"));
    let after_trigrams = t.elapsed();
    dict.add_english_contractions();
    let total = t.elapsed();

    println!("load 50k words      {after_words:?}");
    println!("load bigrams        {:?}", after_bigrams - after_words);
    println!("load trigrams       {:?}", after_trigrams - after_bigrams);
    println!("contractions+caches {:?}", total - after_trigrams);
    println!("total model load    {total:?}");

    // A burst of lookups through a real three-word context
    let contexts = [
        ("as", "soon", "as"),
        ("thank", "you", "for"),
        ("looking", "forward", "to"),
        ("of", "course", "i"),
        ("in", "the", "m"),
        ("it", "is", "imp"),
        ("no", "longer", "a"),
        ("that", "is", "n"),
    ];
    let prefixes = ["a", "as", "i", "t", "l", "n", "th", "f"];

    let n: u32 = 2000;
    let start = Instant::now();
    let mut total_returned = 0usize;
    for i in 0..n {
        let (pp, p, _) = contexts[(i as usize) % contexts.len()];
        let ctx = Context::new(Some(p.to_string()), Some(pp.to_string()));
        let got = dict.suggest(prefixes[(i as usize) % prefixes.len()], &ctx, 5);
        total_returned += got.len();
    }
    let elapsed = start.elapsed();
    println!(
        "\n{n} full suggest() lookups: {:?} total, {:?} each ({} candidates returned)",
        elapsed,
        elapsed / n,
        total_returned
    );
    println!("Worst case is what a keystroke has to fit in: well under a millisecond.");

    // Show what it actually predicts
    println!("\nsample predictions:");
    for (pp, p, expected) in contexts {
        let ctx = Context::new(Some(p.to_string()), Some(pp.to_string()));
        for prefix in prefixes.iter().take(3) {
            let got = dict.suggest(prefix, &ctx, 3);
            println!(
                "  {pp} {p} |{prefix}...  ->  {got:?}   (expected after \"{pp} {p}\": {expected}...)"
            );
        }
    }
}
