#!/usr/bin/env bash
set -euo pipefail

# check-consent-erasure-barrier.sh (#4928)
#
# consent-revoke erasure 차단 chokepoint(GuardedConnection)의 by-construction
# 완전성을 강제하는 lexical CI gate. `scripts/check-architecture-deps.sh` 패턴을
# 따른다.
#
# 불변식(invariant):
#   1. maekon-storage 의 어떤 프로덕션 코드도 raw `Connection`/`GuardedConnection`
#      뮤텍스를 funnel 밖에서 `.lock()` 해서는 안 된다. 유일한 합법 `.lock()` 은
#      GuardedConnection impl 내부(`guarded_connection.rs`)뿐이다. 모든 어댑터는
#      `write_lock()`/`read_lock()`/`retained_write_lock()`/`lock_for_erase()`
#      funnel 을 통해서만 커넥션에 도달한다.
#   2. `connection_arc()` 가 raw `Arc<Mutex<Connection>>` 를 반환하도록 되돌려서는
#      안 된다(`Arc<GuardedConnection>` 만 허용).
#
# 테스트 코드는 제외한다: `*/tests.rs`, `*/test_utils.rs`, `port_contract_tests.rs`,
# 그리고 `#[cfg(test)]` 모듈은 barrier 검사 대상이 아니다(스펙 §3bis CI gate).

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

STORAGE_SRC="crates/maekon-storage/src"
GUARDED_IMPL="$STORAGE_SRC/sqlite/guarded_connection.rs"

echo "[consent-barrier] Verifying GuardedConnection erasure chokepoint (#4928)"

errors=0

# 검사 대상 프로덕션 파일 목록(테스트 파일 제외). macOS bash 3.2 호환을 위해
# mapfile 대신 while-read 루프를 사용한다.
# ── 불변식 1: funnel 밖 raw `.lock()` 금지 ──────────────────────────────
# `conn.lock()` / `self.conn.lock()` / `<...>.conn.lock()` 형태를 잡는다.
# GuardedConnection impl 파일은 내부적으로 parking_lot 뮤텍스를 lock 하므로 면제.
while IFS= read -r f; do
  if [[ "$f" == "$GUARDED_IMPL" ]]; then
    continue
  fi
  # `#[cfg(test)]` 이후 라인은 테스트 모듈이므로 제외하기 위해, 첫 `#[cfg(test)]`
  # 등장 라인까지만 검사한다(스토리지 어댑터는 모두 파일 하단에 테스트 모듈을 둔다).
  cfg_test_line="$(grep -n '#\[cfg(test)\]' "$f" | head -1 | cut -d: -f1 || true)"
  if [[ -n "${cfg_test_line:-}" ]]; then
    scan="$(sed -n "1,$((cfg_test_line - 1))p" "$f")"
  else
    scan="$(cat "$f")"
  fi

  # `.lock()` 호출 중 conn 관련 식별자에 대한 것만 매칭(generic Mutex 가드 오탐 회피).
  # 단일 라인(`conn.lock()`/`self.conn.lock()`)과 멀티라인(`self\n.conn\n.lock()`)
  # 을 모두 잡기 위해, 줄바꿈/공백을 제거한 정규화 텍스트에서 `.conn.lock(` 를 찾는다.
  normalized="$(printf '%s' "$scan" | tr -d '\n' | tr -s ' \t')"
  if printf '%s' "$normalized" | grep -qE '\.conn \.lock\(|\.conn\.lock\(|\bconn\.lock\('; then
    hits="$(printf '%s\n' "$scan" | grep -nE '\.lock\(\)' || true)"
  else
    hits=""
  fi
  if [[ -n "$hits" ]]; then
    echo "[consent-barrier] FORBIDDEN raw conn .lock() outside GuardedConnection funnel in $f:" >&2
    printf '%s\n' "$hits" | sed 's/^/    /' >&2
    errors=$((errors + 1))
  fi
done < <(
  find "$STORAGE_SRC" -name '*.rs' \
    ! -name 'tests.rs' \
    ! -name 'test_utils.rs' \
    ! -name 'port_contract_tests.rs' \
    | sort
)

# ── 불변식 1b (#4928 round-3 FIX A): read 경로로 write-SQL 밀반입 금지 ────
# `ReadGuard` 는 write-capable `&Connection` 을 노출하므로, `read_lock()` 으로 연
# scope 안에서 INSERT/UPDATE/DELETE/REPLACE/CREATE/ALTER/DROP 같은 write-SQL 동사가
# 문자열 리터럴로 등장하면 mutation 이 read funnel 을 우회한 것이다 → 차단.
#
# 접근: 각 `read_lock(` 토큰이 등장한 라인부터 중괄호 균형이 맞을 때까지의
# 영역(enclosing closure/block)을 추출해 write-SQL 동사를 검사한다. 영역 안에서
# write 동사를 만나면 file:line 과 함께 실패한다. 테스트 영역(`#[cfg(test)]` 이후,
# 제외 파일)은 스캔 대상에서 빠진다(불변식 1 과 동일 스코프).
scan_read_lock_write_smuggle() {
  local f="$1"
  local cfg_test_line scan_end
  cfg_test_line="$(grep -n '#\[cfg(test)\]' "$f" | head -1 | cut -d: -f1 || true)"
  if [[ -n "${cfg_test_line:-}" ]]; then
    scan_end=$((cfg_test_line - 1))
  else
    scan_end=0 # 0 = 끝까지
  fi

  # 절대 중괄호 깊이(depth)를 추적한다. `read_lock(` 가 등장하면 그 바인딩의
  # 유효 scope = 등장 시점의 enclosing block 이다(let 바인딩이든 인라인이든).
  # region 은 depth 가 read_lock 등장 시점의 깊이(guard_depth) "미만"으로 떨어질
  # 때 종료된다. region 진행 중 write-SQL 동사가 나타나면 위반이다. 단, region
  # 안에서 명시적 write 경로(`write_lock(`/`retained_write_lock(`/`lock_for_erase(`)
  # 가 열리면 그 write 경로 라인은 합법이므로 면제한다(같은 함수 내 read+write 혼용).
  awk -v limit="$scan_end" -v file="$f" '
    function count(c, s,   n) { n = gsub(c, c, s); return n }
    # POSIX awk 에는 \b 가 없으므로, 앞뒤를 공백으로 패딩해 비-식별자 경계로 매칭한다.
    function has_write(s,   t) {
      t = " " s " "
      return (t ~ /[^A-Za-z_](INSERT|UPDATE|DELETE|REPLACE|CREATE|ALTER|DROP)[^A-Za-z_]/)
    }
    BEGIN { depth = 0; in_region = 0; guard_depth = 0 }
    {
      if (limit > 0 && NR > limit) exit
      raw = $0
      line = raw
      sub(/\/\/.*$/, "", line)   # 라인 코멘트 제거

      # 이 라인이 명시적 write 경로를 열면, 동일 라인의 write-SQL 은 합법.
      is_write_path = (line ~ /(write_lock|retained_write_lock|lock_for_erase)[[:space:]]*\(/)

      starts_read = (line ~ /read_lock[[:space:]]*\(/)

      open_before = depth
      n_open = count("{", line)
      n_close = count("}", line)

      # region 진행 중이면 write-SQL 검사(write 경로 라인은 면제).
      if (in_region && !is_write_path && has_write(line)) {
        printf("%s:%d (read_lock scope)\n", file, NR)
        found = 1
      }

      # 새 read_lock region 진입(아직 region 아닐 때만 시작 — 중첩은 단순화).
      if (!in_region && starts_read) {
        in_region = 1
        guard_depth = open_before  # 이 깊이 미만으로 떨어지면 scope 종료
        # 같은 라인에 write-SQL 이 있으면(읽기 바인딩과 동시) 위반.
        if (!is_write_path && has_write(line)) {
          printf("%s:%d (read_lock scope)\n", file, NR)
          found = 1
        }
      }

      depth += n_open - n_close
      if (in_region && depth < guard_depth) { in_region = 0 }
    }
  ' "$f"
  # awk 는 위반 라인을 stdout 으로만 보고한다(비-zero exit 미사용 — `set -e` 호환).
  # 호출부가 출력 유무로 위반을 판정한다.
}

while IFS= read -r f; do
  if [[ "$f" == "$GUARDED_IMPL" ]]; then
    continue
  fi
  smuggle_hits="$(scan_read_lock_write_smuggle "$f")"
  if [[ -n "$smuggle_hits" ]]; then
    echo "[consent-barrier] FORBIDDEN write-SQL smuggled through a read_lock() scope in $f:" >&2
    echo "                  (ReadGuard exposes a write-capable &Connection — route mutations via write_lock())" >&2
    printf '%s\n' "$smuggle_hits" | sed 's/^/    /' >&2
    errors=$((errors + 1))
  fi
done < <(
  find "$STORAGE_SRC" -name '*.rs' \
    ! -name 'tests.rs' \
    ! -name 'test_utils.rs' \
    ! -name 'port_contract_tests.rs' \
    | sort
)

# ── 불변식 2: connection_arc() 가 GuardedConnection 만 반환 ──────────────
# raw `Arc<Mutex<Connection>>` 반환으로 회귀하면 차단.
if grep -REn 'fn connection_arc\(&self\) -> Arc<Mutex<' "$STORAGE_SRC" >/dev/null 2>&1; then
  echo "[consent-barrier] FORBIDDEN: connection_arc() returns a raw Arc<Mutex<Connection>>" >&2
  echo "                  it MUST return Arc<GuardedConnection> (#4928 chokepoint)." >&2
  grep -REn 'fn connection_arc\(&self\) -> Arc<Mutex<' "$STORAGE_SRC" | sed 's/^/    /' >&2
  errors=$((errors + 1))
fi

# connection_arc() 가 GuardedConnection 을 반환하는지 양성 확인(존재 + 시그니처).
if ! grep -REn 'fn connection_arc\(&self\) -> Arc<GuardedConnection>' "$STORAGE_SRC" >/dev/null 2>&1; then
  echo "[consent-barrier] FORBIDDEN: connection_arc() -> Arc<GuardedConnection> not found." >&2
  echo "                  The chokepoint accessor must exist and return the guarded type." >&2
  errors=$((errors + 1))
fi

if [[ "$errors" -gt 0 ]]; then
  echo "[consent-barrier] FAILED with $errors violation(s)." >&2
  echo "[consent-barrier] All SQLite access in maekon-storage MUST flow through" >&2
  echo "                  GuardedConnection::{write_lock,read_lock,retained_write_lock,lock_for_erase}." >&2
  exit 1
fi

echo "[consent-barrier] OK — no barrier-free connection access outside the funnel."
