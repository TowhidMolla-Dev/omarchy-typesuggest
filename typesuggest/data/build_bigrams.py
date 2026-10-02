#!/usr/bin/env python3
"""Build data/bigrams.tsv (word pairs with counts) from Tatoeba's English sentences.

Input: the Tatoeba English export, eng_sentences.tsv.bz2, from
https://downloads.tatoeba.org/exports/per_language/eng/eng_sentences.tsv.bz2
(sentences are licensed CC BY 2.0 FR, https://tatoeba.org/en/terms_of_use).

Usage: python3 build_bigrams.py eng_sentences.tsv.bz2 en_50k.txt > bigrams.tsv

How the pairs are made:
- sentences are lowercased, ’ becomes ', and split into clauses at punctuation;
  pairs never cross a clause boundary
- words are letters with at most one inner apostrophe (don't, i'm, father's)
- "tom" and "mary", Tatoeba's stand-in names in a large share of sentences, are left out
- each word must be in the word list (en_50k.txt) or be a word from it plus an apostrophe
  ending ('t, 's, 'm, 're, 'll, 've, 'd), and every pair must occur at least twice
- each word keeps its 200 most frequent followers
Output lines are "word<TAB>follower<TAB>count", grouped by word, followers by count.
"""

import bz2
import collections
import re
import sys

MIN_COUNT = 2
MAX_FOLLOWERS = 200
LEFT_OUT = {"tom", "mary", "tom's", "mary's"}
WORD = re.compile(r"[a-z]+(?:'[a-z]+)?")
CLAUSE_BREAK = re.compile(r"[.!?;:,()\"“”—]+")
ENDINGS = ("n't", "'s", "'m", "'re", "'ll", "'ve", "'d")


def load_vocabulary(words_path):
    vocabulary = set()
    with open(words_path, encoding="utf-8") as words:
        for line in words:
            parts = line.split()
            if parts:
                vocabulary.add(parts[0].lower())
    return vocabulary


def make_known(vocabulary):
    def known(word):
        if word in LEFT_OUT:
            return False
        if word in vocabulary:
            return True
        for ending in ENDINGS:
            if word.endswith(ending) and len(word) > len(ending):
                stem = word[: -len(ending)]
                # n't contractions: do+n't, ca+n't (can't), wo+n't (won't)
                if ending == "n't":
                    return stem in vocabulary or stem in ("ca", "wo", "sha")
                return stem in vocabulary
        return False

    return known


def iter_clauses(sentences_path, known):
    """Yield the word tokens of each clause of each sentence.

    Sentences are lowercased, ’ becomes ', and split at punctuation; a clause
    boundary is also a boundary between n-grams, so pairs and triples never
    cross one.
    """
    with bz2.open(sentences_path, "rt", encoding="utf-8") as sentences:
        for line in sentences:
            fields = line.rstrip("\n").split("\t")
            if len(fields) < 3:
                continue
            text = fields[2].lower().replace("’", "'")
            for clause in CLAUSE_BREAK.split(text):
                yield WORD.findall(clause)


def main(sentences_path, words_path):
    known = make_known(load_vocabulary(words_path))

    pairs = collections.Counter()
    for tokens in iter_clauses(sentences_path, known):
        for first, second in zip(tokens, tokens[1:]):
            if known(first) and known(second):
                pairs[(first, second)] += 1

    followers = collections.defaultdict(list)
    for (first, second), count in pairs.items():
        if count >= MIN_COUNT:
            followers[first].append((count, second))

    out = sys.stdout
    for first in sorted(followers):
        ranked = sorted(followers[first], key=lambda item: (-item[0], item[1]))
        for count, second in ranked[:MAX_FOLLOWERS]:
            out.write(f"{first}\t{second}\t{count}\n")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: build_bigrams.py eng_sentences.tsv.bz2 en_50k.txt > bigrams.tsv")
    main(sys.argv[1], sys.argv[2])
