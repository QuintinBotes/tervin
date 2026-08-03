#!/usr/bin/env bash
#
# Build a scratch project worth pointing Tervin at.
#
# Manual testing against this repo is a poor test: it is huge, it is the thing
# being tested, and an agent let loose in it can edit Tervin itself. This makes a
# small, self-contained, throwaway repository that still has enough in it for
# Blocks, Threads, the file explorer and a Plan handoff to have something real to
# chew on — including a genuinely failing test, so "fix this" is a real request.
#
#   ./scripts/testbed.sh            # create it, print the path
#   ./scripts/testbed.sh --clean    # remove it
#
set -euo pipefail

BED="${TERVIN_TESTBED:-$HOME/tervin-testbed}"

if [[ "${1:-}" == "--clean" ]]; then
  rm -rf "$BED"
  echo "removed $BED"
  exit 0
fi

if [[ -e "$BED" ]]; then
  echo "already exists: $BED"
  echo "run with --clean first if you want a fresh one"
  exit 0
fi

mkdir -p "$BED/src" "$BED/docs"
cd "$BED"

cat > README.md <<'EOF'
# widget-service

A deliberately small service, used to exercise Tervin by hand.

Nothing here is real. It exists so Blocks, Threads, the file explorer and Plan
handoff have something to act on that is not Tervin's own source tree.
EOF

cat > AGENTS.md <<'EOF'
# Instructions for agents working here

- This is a throwaway test project. Nothing in it ships.
- Prefer small, surgical edits.
- The suite is stdlib `unittest`. Run it with `python3 -m unittest -v`.
EOF

cat > src/widget.py <<'EOF'
"""A widget, and the arithmetic it is bad at."""


def price_with_tax(price, rate):
    # Deliberately wrong: the rate is applied twice.
    taxed = price + (price * rate)
    return taxed + (price * rate)


def discount(price, percent):
    if percent < 0 or percent > 100:
        raise ValueError("percent must be between 0 and 100")
    return price * (1 - percent / 100)


def describe(name, price):
    return f"{name}: {price:.2f}"
EOF

cat > src/inventory.py <<'EOF'
"""Stock tracking, such as it is."""

from collections import defaultdict


class Inventory:
    def __init__(self):
        self._counts = defaultdict(int)

    def add(self, sku, count=1):
        self._counts[sku] += count

    def remove(self, sku, count=1):
        self._counts[sku] -= count
        return self._counts[sku]

    def total(self):
        return sum(self._counts.values())
EOF

# stdlib unittest, so the suite runs on a bare machine with no pip install.
cat > test_widget.py <<'EOF'
import unittest

from src.widget import price_with_tax, discount, describe


class TestWidget(unittest.TestCase):
    def test_price_with_tax(self):
        # This one fails: price_with_tax applies the rate twice.
        self.assertEqual(price_with_tax(100.0, 0.1), 110.0)

    def test_discount(self):
        self.assertEqual(discount(100.0, 25), 75.0)

    def test_describe(self):
        self.assertEqual(describe("bolt", 1.5), "bolt: 1.50")


if __name__ == "__main__":
    unittest.main()
EOF

touch src/__init__.py

cat > docs/NOTES.md <<'EOF'
# Notes

The tax calculation has been wrong since the first commit. Nobody has fixed it
because nobody reads the tests.
EOF

cat > .gitignore <<'EOF'
__pycache__/
*.pyc
.pytest_cache/
EOF

git init -q
git add -A
git -c user.email=testbed@example.com -c user.name="Testbed" \
  commit -q -m "Initial commit: a widget service with one real bug"

# A dirty working tree, so the Git panel has something to show.
printf '\ndef restock(self, sku, count):\n    self.add(sku, count)\n' >> src/inventory.py
printf 'scratch file, untracked\n' > scratch.txt

echo "$BED"
