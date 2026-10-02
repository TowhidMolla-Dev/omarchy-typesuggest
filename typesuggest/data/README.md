# Data files

TypeSuggest's code is MIT licensed (see [`LICENSE`](../LICENSE)). The data files in this
folder, which are also built into the `typesuggest` binary, keep their own licenses:

| File | Contents | Source | License |
|---|---|---|---|
| `en_50k.txt` | The 50,000 most frequent English words with counts | [FrequencyWords](https://github.com/hermitdave/FrequencyWords) by Hermit Dave, `content/2018/en/en_50k.txt`, built from [OpenSubtitles](https://www.opensubtitles.org) 2018 | [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/) |
| `bigrams.tsv` | About 320,000 word pairs with counts | Built by [`build_bigrams.py`](build_bigrams.py) from the English sentences written by [Tatoeba](https://tatoeba.org) contributors (export of 2026-09-26) | [CC BY 2.0 FR](https://creativecommons.org/licenses/by/2.0/fr/) |
| `trigrams.tsv` | About 319,000 word triples with counts | Built by [`build_trigrams.py`](build_trigrams.py) from the same Tatoeba export | [CC BY 2.0 FR](https://creativecommons.org/licenses/by/2.0/fr/) |

## Changes

- `en_50k.txt` is unmodified. When loading it, TypeSuggest skips entries that are not words
  (endings the source split off, such as `'s` and `'t`, and abbreviations such as `mr.`),
  drops contraction halves and apostrophe-less spellings (`didn`, `dont`, `im`), and adds
  English contractions whose frequencies are derived from this list's counts (the
  `CONTRACTIONS` table in [`src/dict.rs`](../src/dict.rs)). That derived table is also under
  CC BY-SA 4.0.
- `bigrams.tsv` counts word pairs in Tatoeba's sentences: lowercased, never across
  punctuation, without the stand-in names "Tom" and "Mary", limited to words from
  `en_50k.txt` (and their contractions), pairs seen at least twice, and at most 200 followers
  per word. [`build_bigrams.py`](build_bigrams.py) reproduces it exactly.
- `trigrams.tsv` counts word triples in the same sentences, with the same word filter and
  clause boundaries. A triple must be seen at least three times (a pair needs two) and each
  pair of words keeps its 10 most frequent followers (a word keeps 200), since a triple is
  consulted to choose between a handful of candidates rather than to enumerate them.
  [`build_trigrams.py`](build_trigrams.py) reproduces it exactly, and shares its tokenizing
  with `build_bigrams.py` so the two tables cannot drift apart.

## Tools

The scripts write their table to standard output:

```bash
python3 build_bigrams.py  eng_sentences.tsv.bz2 en_50k.txt > bigrams.tsv
python3 build_trigrams.py eng_sentences.tsv.bz2 en_50k.txt > trigrams.tsv
```

`make_eval_data.py` measures a change to ranking honestly: it splits Tatoeba's sentences
deterministically by sentence id, 90% train and 10% evaluation, and builds its n-gram tables
from the training half only. Scoring a ranking on the evaluation half then measures how well it
generalizes rather than how well it memorizes, which a table scored against its own sentences
would trivially do.

```bash
python3 make_eval_data.py eng_sentences.tsv.bz2 en_50k.txt ../evaldata
cargo run --release --example ranking_eval -- ../evaldata
cargo run --release --example bench        # model load time and lookup latency
```
