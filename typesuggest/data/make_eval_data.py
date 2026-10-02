#!/usr/bin/env python3
"""Build train/eval split data so ranking changes can be measured honestly.

Usage: python3 make_eval_data.py eng_sentences.tsv.bz2 en_50k.txt outdir

Splits Tatoeba's English sentences deterministically by sentence id (90% train,
10% eval). The n-gram tables are rebuilt from the TRAIN half only, so evaluating
a ranking on the eval half measures generalization rather than memorization.

Writes:
  outdir/bigrams.tsv, outdir/trigrams.tsv   tables from the train half
  outdir/cases_prefix{N}.tsv               eval cases at several prefix lengths
    each line: prev<TAB>prev_prev<TAB>prefix<TAB>target
"""

import bz2
import collections
import hashlib
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from build_bigrams import iter_clauses, load_vocabulary, make_known
from build_trigrams import MIN_COUNT as TRI_MIN_COUNT, MAX_FOLLOWERS as TRI_MAX_FOLLOWERS
from build_bigrams import MAX_FOLLOWERS as BI_MAX_FOLLOWERS

WORD = re.compile(r"[a-z]+(?:'[a-z]+)?")
EVAL_PERCENT = 10
PREFIX_LENGTHS = (1, 3)


def is_eval(sentence_id):
    h = hashlib.sha256(sentence_id.encode()).hexdigest()
    return int(h[:8], 16) % 100 < EVAL_PERCENT


def write_tables(train_pairs, train_triples, outdir):
    with open(os.path.join(outdir, "bigrams.tsv"), "w") as f:
        for first in sorted(train_pairs):
            ranked = sorted(train_pairs[first].items(), key=lambda i: (-i[1], i[0]))
            for count, second in ranked[:BI_MAX_FOLLOWERS]:
                f.write(f"{first}\t{second}\t{count}\n")

    with open(os.path.join(outdir, "trigrams.tsv"), "w") as f:
        for key in sorted(train_triples):
            ranked = sorted(
                (
                    (n, c)
                    for c, n in train_triples[key].items()
                    if n >= TRI_MIN_COUNT
                ),
                key=lambda i: (-i[0], i[1]),
            )
            if not ranked:
                continue
            for count, third in ranked[:TRI_MAX_FOLLOWERS]:
                f.write(f"{key[0]}\t{key[1]}\t{third}\t{count}\n")


def main(sentences_path, words_path, outdir):
    os.makedirs(outdir, exist_ok=True)
    known = make_known(load_vocabulary(words_path))

    train_pairs = collections.defaultdict(collections.Counter)
    train_triples = collections.defaultdict(collections.Counter)
    cases = {n: [] for n in PREFIX_LENGTHS}

    with bz2.open(sentences_path, "rt", encoding="utf-8") as f:
        for line in f:
            fields = line.rstrip("\n").split("\t")
            if len(fields) < 3:
                continue
            held_out = is_eval(fields[0])

            for tokens in iter_clauses_from_text(fields[2]):
                # Tables come from the train half only
                if not held_out:
                    for a, b in zip(tokens, tokens[1:]):
                        if known(a) and known(b):
                            train_pairs[a][b] += 1
                    for a, b, c in zip(tokens, tokens[1:], t2(tokens)):
                        if known(a) and known(b) and known(c):
                            train_triples[(a, b)][c] += 1
                # Cases come from the eval half
                else:
                    for i in range(2, len(tokens)):
                        target = tokens[i]
                        if not (known(target) and known(tokens[i - 1]) and known(tokens[i - 2])):
                            continue
                        for n in PREFIX_LENGTHS:
                            if len(target) > n:
                                cases[n].append(
                                    (tokens[i - 1], tokens[i - 2], target[:n], target)
                                )

    write_tables(train_pairs, train_triples, outdir)
    for n, rows in cases.items():
        with open(os.path.join(outdir, f"cases_prefix{n}.tsv"), "w") as f:
            for prev, prev_prev, prefix, target in rows:
                f.write(f"{prev}\t{prev_prev}\t{prefix}\t{target}\n")
        print(f"prefix length {n}: {len(rows)} cases")

    print("train contexts:", len(train_pairs), len(train_triples))


def t2(tokens):
    return tokens[2:]


def iter_clauses_from_text(text):
    from build_bigrams import CLAUSE_BREAK

    for clause in CLAUSE_BREAK.split(text.lower().replace("’", "'")):
        yield WORD.findall(clause)


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit("usage: make_eval_data.py eng_sentences.tsv.bz2 en_50k.txt outdir")
    main(sys.argv[1], sys.argv[2], sys.argv[3])