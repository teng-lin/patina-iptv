#!/usr/bin/env python3
"""Writes the interface languages' messages as one package file per language,
for iptv-app's `browser-package` feature (iptv-presentation's `external-catalog`):
`<tag>.tsv` with a line per message, its English source and its text
separated by a tab, and `en.tsv` with the sources alone.

    catalog.py MESSAGES_TSV I18N_RS OUT_DIR

The tags, in the catalog's column order, are read from `LANGUAGES` in
iptv-presentation's i18n.rs; a catalog with another number of columns fails.
"""
import os
import re
import sys


def tags(i18n_rs):
    source = open(i18n_rs, encoding="utf-8").read()
    table = re.search(r"pub const LANGUAGES: &\[Language\] = &\[(.*?)\n\];", source, re.S)
    if not table:
        raise SystemExit(f"{i18n_rs}: no LANGUAGES table")
    return re.findall(r'tag: "([^"]+)"', table.group(1))


def main(argv):
    if len(argv) != 4:
        raise SystemExit(__doc__)
    messages, i18n_rs, out = argv[1:]
    languages = tags(i18n_rs)
    files = {tag: [] for tag in languages}
    for number, line in enumerate(open(messages, encoding="utf-8").read().splitlines(), 1):
        if not line:
            continue
        columns = line.split("\t")
        if len(columns) != len(languages):
            raise SystemExit(f"{messages}:{number}: {len(columns)} columns, {len(languages)} languages")
        files[languages[0]].append(columns[0])
        for tag, text in zip(languages[1:], columns[1:]):
            files[tag].append(f"{columns[0]}\t{text}")
    os.makedirs(out, exist_ok=True)
    for tag, lines in files.items():
        with open(os.path.join(out, f"{tag}.tsv"), "w", encoding="utf-8", newline="\n") as f:
            f.write("\n".join(lines) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
