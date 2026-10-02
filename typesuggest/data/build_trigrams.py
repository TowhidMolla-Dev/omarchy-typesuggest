#!/usr/bin/env python3
"""Build data/trigrams.tsv (word triples with counts) from Tatoeba's English sentences.

Shares its tokenizing with build_bigrams.py, so triples are counted over exactly
the same words and clause boundaries as the pairs in bigrams.tsv.

Input: the Tatoeba English export, eng_sentences.tsv.bz2, from
https://downloads.tatoeba.org/exports/per_language/eng/eng_sentences.tsv.bz2
(sentences are licensed CC BY 2.0 FR, https://tatoeba.org/en/terms_of_use).

Usage: python3 build_trigrams.py eng_sentences.tsv.bz2 en_50k.txt > trigrams.tsv

How the triples are made: the same as the pairs (lowercased, never across a
clause boundary, "tom" and "mary" left out, words limited to en_50k.txt and its
contractions), with two differences:

- a triple must occur at least MIN_COUNT (3) times, where a pair needs 2: triples
  are sparser, and a triple seen once is as likely to be one duplicated sentence
  as a real phrase
- each pair of words keeps its MAX_FOLLOWERS (10) most frequent followers, where
  a word keeps 200. A triple is consulted to pick the best of a handful of
  candidates, not to enumerate them, so a short list is enough.

Most surviving triples are deterministic: in Tatoeba the top follower of a pair
of words is very often its only follower seen three times or more. That is the
point of the table -- "as soon as" or "of course i" are decided by the two words
before the caret, and the bigram table alone cannot see them.

Output lines are "w1<TAB>w2<TAB>w3<TAB>count", grouped by w1 then w2, followers
by count.
"""

import collections
import sys

from build_bigrams import iter_clauses, load_vocabulary, make_known

MIN_COUNT = 3
MAX_FOLLOWERS = 10


def main(sentences_path, words_path):
    known = make_known(load_vocabulary(words_path))

    triples = collections.defaultdict(collections.Counter)
    for tokens in iter_clauses(sentences_path, known):
        for first, second, third in zip(tokens, tokens[1:], tokens[2:]):
            if known(first) and known(second) and known(third):
                triples[(first, second)][third] += 1

    out = sys.stdout
    for first, second in sorted(triples):
        ranked = sorted(
            (
                (count, third)
                for third, count in triples[(first, second)].items()
                if count >= MIN_COUNT
            ),
            key=lambda item: (-item[0], item[1]),
        )
        if not ranked:
            continue
        for count, third in ranked[:MAX_FOLLOWERS]:
            out.write(f"{first}\t{second}\t{third}\t{count}\n")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(
            "usage: build_trigrams.py eng_sentences.tsv.bz2 en_50k.txt > trigrams.tsv"
        )
    main(sys.argv[1], sys.argv[2])
