#!/usr/bin/env python3
"""Sincronía de las traducciones (`X.en.md` ↔ `X.md`).

Cada traducción lleva al pie un marcador `<!-- sync: sha256:<12 hex> -->` con el hash del ORIGINAL
del que se tradujo. `tests/docs_i18n.rs` lo contrasta con el hash actual del original: si el
original cambió y la traducción no se revisó, el CI avisa. Este script lista el estado
(`--check`) o refresca los marcadores tras revisar una traducción (`--update [archivo…]`).
"""
import hashlib, pathlib, re, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
MARK = re.compile(r"<!-- sync: sha256:([0-9a-f]{12}) -->")


def pairs():
    for en in sorted(list(ROOT.glob("*.en.md")) + list((ROOT / "docs").glob("*.en.md"))):
        yield en, en.with_name(en.name.replace(".en.md", ".md"))


def digest(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()[:12]


def main(argv):
    update = "--update" in argv
    only = {pathlib.Path(a).resolve() for a in argv if not a.startswith("--")}
    stale = 0
    for en, es in pairs():
        if only and en.resolve() not in only:
            continue
        want = digest(es)
        text = en.read_text(encoding="utf-8")
        m = MARK.search(text)
        have = m.group(1) if m else None
        if update:
            new = f"<!-- sync: sha256:{want} -->"
            text = MARK.sub(new, text) if m else text.rstrip("\n") + "\n\n" + new + "\n"
            en.write_text(text, encoding="utf-8")
            print(f"actualizado {en.relative_to(ROOT)} → {want}")
        elif have == want:
            print(f"ok      {en.relative_to(ROOT)}")
        else:
            stale += 1
            print(f"DESFASE {en.relative_to(ROOT)}: marcador {have}, original {want} — revisa la traducción y corre tools/docs_sync.py --update")
    return 1 if stale else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
