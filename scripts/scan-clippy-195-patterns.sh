#!/usr/bin/env bash
# scan-clippy-195-patterns.sh — Pre-flight grep scan for Rust 1.95 clippy
# lints that surface in CI but are easy to miss when the local toolchain lags.
#
# Background: CI uses `dtolnay/rust-toolchain@stable` which auto-picks the
# current stable. When stable ticks over, fresh lint errors appear. The
# 1.94 → 1.95 bump caused PRs #425/#426 to take 3 push rounds to clear
# because local clippy did not catch the new patterns.
#
# This is a fast (sub-second) grep-based pre-flight check that only flags
# patterns with **precise** signatures — i.e. signatures whose grep regex
# tracks clippy's actual rule closely enough to keep false-positive rate
# at or near zero on the current tree.
#
# Patterns covered:
#   1. unnecessary_sort_by         — REVERSE sort with `b.field.cmp(&a.field)`.
#                                    Detected via perl backreference so ascending
#                                    sorts (`a.field.cmp(&b.field)`) are skipped.
#   2. field_reassign_with_default — `let mut x = X::default();` followed by
#                                    one or more `x.field = ...` assignments.
#                                    Multi-line slurp + name-bound regex so
#                                    method calls like `x.method()` aren't flagged.
#                                    This is precisely the pattern that bit
#                                    PR #450 in feature-gated test code, where
#                                    `cargo clippy --workspace` (no `--features`)
#                                    never compiled the offending block.
#
# Patterns NOT covered (regex too noisy without context awareness — rely on
# `cargo clippy` for these):
#   - manual_while_let_some  (false positives in `tokio::select!` loops and
#                             loops with prelude statements before the let-else)
#   - collapsible_match
#   - items_after_test_module
#   - ptr_arg                (clippy applies usage-site heuristics this script
#                             cannot replicate)
#
# When you do bump the local toolchain, follow up with the broader
# `cargo clippy --workspace --all-targets -- -D warnings` to catch the
# patterns this script intentionally skips.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

declare -i exit_code=0

# 1. unnecessary_sort_by (Rust 1.95)
#    `v.sort_by(|a, b| b.field.cmp(&a.field))` → use `sort_by_key + Reverse`.
#    Perl regex with backreference: capture the two closure params, then
#    require body to be `<param2>.field.cmp(&<param1>.field)` — this matches
#    only REVERSE sorts and skips ascending ones.
UNNECESSARY_SORT_HITS=$(
    find crates src-tauri \
        -path '*/target' -prune -o \
        -name '*.rs' -type f -print 2>/dev/null \
        | xargs perl -nle '
            BEGIN { $found = 0 }
            if (/\.sort_by\(\|([a-z_][a-z_0-9]*),\s*([a-z_][a-z_0-9]*)\|\s*\2\.[a-z_0-9]+\.cmp\(&\1\./) {
                print "$ARGV:$.:$_";
                $found = 1;
            }
            END { exit($found ? 1 : 0) }
        ' 2>/dev/null
)
if [ -n "$UNNECESSARY_SORT_HITS" ]; then
    echo "[FAIL] unnecessary_sort_by — REVERSE-sort with .cmp() (Rust 1.95):"
    printf '%s\n' "$UNNECESSARY_SORT_HITS" | sed 's/^/   /'
    echo ""
    exit_code=1
fi

# 2. field_reassign_with_default
#    `let mut x = X::default(); x.f = ...;` → `X { f: ..., ..Default::default() }`.
#    Perl slurp-then-scan with INDENT MATCHING: the field assignment must
#    appear at the SAME indent level as the `let mut`. This excludes
#    conditional fills inside `match` arms or `if` branches (which clippy
#    does NOT flag because the runtime branching can't be inlined into a
#    struct literal). Method calls (`<name>.method()`) and bare reads
#    (`return <name>.x;`) are skipped because the regex requires `=`
#    followed by a non-`=` byte (rules out `==`).
FIELD_REASSIGN_HITS=$(
    find crates src-tauri \
        -path '*/target' -prune -o \
        -name '*.rs' -type f -print 2>/dev/null \
        | xargs perl -e '
            use strict; use warnings;
            my $any_hit = 0;
            for my $file (@ARGV) {
                open my $fh, "<", $file or next;
                local $/ = undef;
                my $content = <$fh>;
                close $fh;

                # Match: indent + "let mut <name> = <Type>::default();" + 0..5 follow lines
                while ($content =~ /^([ \t]*)let mut ([a-z_][a-z_0-9]*)\s*=\s*[A-Z][A-Za-z_0-9:]*::default\(\)\s*;[ \t]*\n((?:[^\n]*\n){0,5})/gm) {
                    my $indent = $1;
                    my $name = $2;
                    my $body = $3;
                    # Same-indent guard: `<indent><name>.<field> = <expr>` (not `==`).
                    if ($body =~ /^\Q$indent\E\Q$name\E\.[a-z_][a-z_0-9]*\s*=\s*[^=]/m) {
                        my $matched_text = $&;
                        my $match_pos = pos($content) - length($matched_text) - length($body);
                        my $line_no = (substr($content, 0, $match_pos) =~ tr/\n//) + 1;
                        my @lines = split /\n/, $matched_text;
                        my $first_line = $lines[0] // "";
                        print "${file}:${line_no}: ${first_line}\n";
                        $any_hit = 1;
                    }
                }
            }
            exit($any_hit ? 1 : 0);
        ' 2>/dev/null
)
if [ -n "$FIELD_REASSIGN_HITS" ]; then
    echo "[FAIL] field_reassign_with_default — let mut + ::default() + field assigns:"
    printf '%s\n' "$FIELD_REASSIGN_HITS" | sed 's/^/   /'
    echo ""
    exit_code=1
fi

if [ "$exit_code" -ne 0 ]; then
    cat <<'EOF'
============================================================
Detected likely Rust 1.95 clippy violation(s).

Quick fixes:
  unnecessary_sort_by:
    v.sort_by(|a, b| b.x.cmp(&a.x));
                ↓
    v.sort_by_key(|x| std::cmp::Reverse(x.x));

  field_reassign_with_default:
    let mut x = Foo::default();
    x.field1 = ...;
    x.field2 = ...;
                ↓
    let x = Foo { field1: ..., field2: ..., ..Foo::default() };

If a flagged line is a known false positive, run
  cargo clippy --workspace --all-targets -- -D warnings
to confirm. cargo clippy is authoritative; this scan exists only to
shortcut CI iteration cost when the local toolchain trails stable OR
when feature-gated code paths are missed by the default workspace
sweep.
============================================================
EOF
fi

exit "$exit_code"
