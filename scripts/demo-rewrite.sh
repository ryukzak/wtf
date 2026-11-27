#!/usr/bin/env bash
# Builds a repository to try wtf's Rewrite on: a few branches, named for what
# to try, whose commit messages say what to do. Each works on files of its
# own, so they don't get in each other's way.
#
#   scripts/demo-rewrite.sh [path]      default: ~/wtf-rewrite-demo
#
# It refuses to touch a path that exists; delete it to build it again. A
# second worktree goes next to it, at <path>-wt.
set -euo pipefail

repo="${1:-$HOME/wtf-rewrite-demo}"
wt="$repo-wt"
for path in "$repo" "$wt"; do
    if [ -e "$path" ]; then
        echo "$path exists; delete it first (rm -rf \"$path\")" >&2
        exit 1
    fi
done

mkdir -p "$repo"
cd "$repo"
git init -q -b main
git config user.name "Demo"
git config user.email "demo@example.com"

# The clock the commits are dated by: an hour per branch apart, try/1
# newest, so the graph, newest first, lists them 1, 2, 3… Each commit a
# minute after the one before.
start=$(( $(date +%s) - 8 * 3600 ))
now=$start
# write FILE: its content from stdin.
write() { mkdir -p "$(dirname "$1")"; cat > "$1"; }
# commit MESSAGE: everything changed, as one commit, dated by the clock.
commit() {
    now=$((now + 60))
    git add -A
    GIT_AUTHOR_DATE="$now +0000" GIT_COMMITTER_DATE="$now +0000" git commit -q -m "$1"
}
# branch NAME [FROM]: a try/N branch sets the clock for its scenario.
branch() {
    git checkout -q -b "$1" "${2:-main}"
    if [[ "$1" =~ ^try/([0-9]+) ]]; then
        now=$((start + (7 - 10#${BASH_REMATCH[1]}) * 3600))
    fi
}

# ---------------------------------------------------------------- main
write TRY.md <<'EOF'
# Trying Rewrite

Open this repository in wtf. Each `try/…` branch is something to try; its
commit messages say what to do.

With the Rewrite tab closed, a change that goes through cleanly is done at
once, with Undo in the notice at the bottom; one with conflicts opens the
Rewrite tab. With the tab open first, changes only wait there as steps until
Apply: try 2 again that way.

1 cherry-pick: clean, and with conflicts to resolve
2 tidy up a branch: squash, drop, reword, edit
3 reorder: one move clean, one that conflicts
4 rebase onto main, with a conflict
5 cherry-pick elsewhere: a branch with no worktree, another worktree

To start over: delete this folder and its -wt sibling, run the script again.
EOF
write parser.js <<'EOF'
export function parse(src) {
  const tokens = lex(src);
  if (tokens.length === 0) {
    return { kind: "empty" };
  }
  return value(tokens, 0);
}

function value(t, i) {
  switch (t[i].kind) {
    case "num": return num(t, i);
    case "str": return str(t, i);
  }
}
EOF
write config.json <<'EOF'
{
  "name": "demo",
  "port": 8080,
  "debug": false
}
EOF
write math.js <<'EOF'
export function total(values) {
  return values.reduce((a, b) => a + b, 0);
}
EOF
write greet.js <<'EOF'
export function greet(name) {
  return "Hi, " + name;
}
EOF
commit "Start: the demo app"
base=$(git rev-parse HEAD)

# A branch with no worktree, for 5.
git branch release/1.0

# ------------------------------------------------------------ 1: cherry-pick
branch try/1-cherry-pick "$base"
write parser.js <<'EOF'
export function parse(src) {
  const tokens = lex(src);
  if (!tokens.length) {
    throw new ParseError("empty");
  }
  return value(tokens, 0);
}

function value(t, i) {
  switch (t[i].kind) {
    case "num": return num(t, i);
    case "str": return str(t, i);
  }
}
EOF
commit "TRY: right-click the section header → Cherry-pick 3: this step conflicts — take a side in three columns"
write parser.test.js <<'EOF'
test("parses a number", () => {
  expect(parse("1")).toEqual({ kind: "num", value: 1 });
});
EOF
commit "TRY: alone, Cherry-pick applies at once (Undo in the notice); in the 3, it's 'likely clean' until step 1 is resolved"
write parser.js <<'EOF'
export function parse(src) {
  const tokens = lex(src);
  if (!tokens.length) {
    throw new ParseError("empty");
  }
  return value(tokens, 0);
}

function value(t, i) {
  switch (t[i].kind) {
    case "num": return number(t[i]);
    case "str": return string(t[i]);
  }
}
EOF
commit "TRY: conflicts too — press Edit lines and type the result yourself, then Apply"

# ------------------------------------------------------------- 2: tidy up
branch try/2-tidy-up "$base"
write cart.js <<'EOF'
export function addItem(cart, item) {
  return [...cart, item];
}
EOF
commit "Add a cart"
write cart.js <<'EOF'
export function addItem(cart, item) {
  return [...cart, item];
}

export function removeItem(cart, id) {
  return cart.filter((item) => item.id !== id);
}
EOF
commit "TRY: right-click → Squash into Previous — joins 'Add a cart', both messages kept (or Fixup: this one dropped)"
write report.js <<'EOF'
export function report(rows) {
  console.log("DEBUG", rows);
  return rows.length + " rows";
}
EOF
commit "TRY: right-click → Drop — a debug print, gone at once"
write date.js <<'EOF'
export function today() {
  return new Date().toISOString().slice(0, 10);
}
EOF
commit "fix stuff (TRY: right-click → Reword… — write a real message in the middle, then Apply)"
write greet.js <<'EOF'
export function greet(name) {
  return "Helo, " + name;
}
EOF
commit "TRY: right-click → Edit… — click greet.js in the middle, fix 'Helo', then Apply"

# ------------------------------------------------------------- 3: reorder
branch try/3-reorder "$base"
write steps/second.txt <<<"second"
commit "TRY: right-click → Move Down — belongs after 'First step'; clean, they touch different files"
write steps/first.txt <<<"first"
commit "First step"
write math.js <<'EOF'
export function sum(values) {
  return values.reduce((a, b) => a + b, 0);
}
EOF
commit "Rename total to sum"
write math.js <<'EOF'
export function sum(values, start = 0) {
  return values.reduce((a, b) => a + b, start);
}
EOF
commit "TRY: right-click → Move Down — it changes the renamed line, so before the rename it conflicts"

# -------------------------------------------------------------- 4: rebase
branch try/4-rebase "$base"
write notes.md <<<"Notes that main doesn't touch."
commit "Add notes (rebases cleanly)"
write config.json <<'EOF'
{
  "name": "demo",
  "port": 9090,
  "debug": false
}
EOF
commit "TRY: right-click the branch label try/4-rebase → Rebase onto main — main changed the same port line: resolve it"

# ---------------------------------------------------- 5: cherry-pick elsewhere
branch try/5-elsewhere "$base"
write fix.js <<<"export const fixed = true;"
commit "TRY: Cherry-pick into Branch… → release/1.0 (no checkout), or Cherry-pick ⌂ wtf-rewrite-demo-wt (the other worktree)"

# ------------------------------------------ main moves on: what 1 and 4 hit
git checkout -q main
now=$((start + 7 * 3600))
write parser.js <<'EOF'
export function parse(src) {
  const tokens = lex(src);
  if (tokens.length === 0) {
    return { kind: "empty", at: 0 };
  }
  return value(tokens, 0);
}

function value(t, i) {
  switch (t[i].kind) {
    case "num": return num(t, i, 10);
    case "str": return str(t, i);
  }
}
EOF
commit "Main: empty input returns a node; numbers in base 10 (what 1 conflicts with)"
write config.json <<'EOF'
{
  "name": "demo",
  "port": 3000,
  "debug": false
}
EOF
commit "Main: port 3000 (what 4 conflicts with)"

# The other worktree, on a branch of its own.
git worktree add -q -b demo-worktree "$wt" main

echo "Built $repo, with a second worktree at $wt."
echo "Open it: open -a wtf \"$repo\"   (or: wtf \"$repo\")"
