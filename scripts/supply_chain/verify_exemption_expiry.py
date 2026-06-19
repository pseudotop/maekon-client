#!/usr/bin/env python3
"""
cargo-vet 면제 항목 만료일 검증 스크립트 (F-SC-C23-02)

cargo-vet 면제 항목은 무기한으로 쌓이는 경향이 있다. 이 스크립트는
notes 필드에 파싱 가능한 `review-by: YYYY-MM-DD` 형식이 포함된 항목을
검사하여 만료된 항목을 보고한다.

사용법:
    python scripts/supply_chain/verify_exemption_expiry.py [--fail-on-expired] [config_path]

옵션:
    --fail-on-expired   만료 항목 존재 시 exit code 1 반환 (CI 하드 게이트용)
                        미지정 시 경고만 출력하고 exit code 0 반환 (소프트 경고)
    config_path         공급망 설정 파일 경로 (기본: clients/maekon-client/supply-chain/config.toml)

경고 전용 모드(기본)는 기존 항목(~635개, 면제 마이그레이션에 따라 변동)을
한 번에 차단하지 않기 위해 사용한다.
--fail-on-expired 는 신규 항목 검증 의무 적용 후 CI gate로 활성화할 것.

F-SC-C38-01: 기존 bespoke 라인 파서는 fail-open 버그가 있었다.
  (1) notes 가 triple-quoted multiline TOML 문자열이면 `review-by:` 가
      notes 시작 줄(triple-quote) 다음 줄에 위치하여 라인 파서가 인식하지 못했고,
  (2) 정규식이 콜론 직후 공백(`review-by: `)을 허용하지 않았다.
  결과적으로 전체 면제 항목(~635개, 면제 마이그레이션에 따라 변동) 중 5개만
  인식(fail-open) -> gate 활성화 시 나머지가 무검증 통과.
  수정: Python 3.13 stdlib tomllib 로 전체 notes 값을 한 문자열로 읽고,
  공백 허용 정규식 `review-by:\\s*(\\d{4}-\\d{2}-\\d{2})` 적용.
  추가: 파싱된 review_by 수 == 파일 내 'review-by' 부분문자열 수 invariant 검증
  (불일치 시 fail-loud — fail-open 재발 차단).

OOS-TBD-CARGO-VET-EXEMPTION-EXPIRY-TRIAGE: 전체 항목 triage (~635개, 변동 — F-SC-C39-01: count-agnostic 표기로 통일)
"""

import sys
import re
import tomllib
import argparse
from datetime import date, timedelta
from pathlib import Path


# notes 필드에서 review-by 날짜를 추출하는 정규식 (콜론 뒤 공백 허용 — F-SC-C38-01)
# 예: "review-by: 2026-08-24" (multiline notes) 또는 "review-by:2026-08-24" (inline notes)
_REVIEW_BY_RE = re.compile(r"review-by:\s*(\d{4}-\d{2}-\d{2})")

# 파싱 커버리지 invariant 검증용 — notes 파싱과 무관하게 파일 원문에 등장하는
# 'review-by' 토큰 수를 세어, tomllib 파싱 결과와 대조한다 (fail-open 재발 차단).
_REVIEW_BY_TOKEN = "review-by"

# deny.toml 주석 스캔용 — deny.toml 은 cargo-deny 설정으로 [[exemptions]] 테이블이
# 없고 Review-by 약속이 주석에만(대문자 'Review-by:') 존재하므로, config.toml 의
# tomllib 파싱 경로와 별개로 case-insensitive 정규식으로 dated 항목만 추출한다 (F-SC-C41-05).
_DENY_REVIEW_BY_RE = re.compile(r"review-by:\s*(\d{4}-\d{2}-\d{2})", re.IGNORECASE)


def parse_exemptions(config_path: Path) -> list[dict]:
    """tomllib 로 [[exemptions.<crate>]] 배열-of-테이블을 파싱한다 (F-SC-C38-01).

    tomllib(Python 3.11+ stdlib)는 triple-quoted multiline notes 값을 단일
    문자열로 정확히 읽으므로, 기존 라인 파서가 놓치던 멀티라인 review-by 날짜를
    빠짐없이 인식한다. 외부 의존성도 추가하지 않는다.

    반환 항목 형태: {"crate", "version", "criteria", "notes", "review_by"?, "review_by_error"?}
    (lineno 는 tomllib 가 제공하지 않으므로, 보고 시 crate@version 으로 식별한다.)
    """
    with config_path.open("rb") as f:
        data = tomllib.load(f)

    raw_exemptions = data.get("exemptions", {})
    exemptions: list[dict] = []

    # [[exemptions.<crate>]] 는 crate 이름별로 항목 리스트가 된다.
    for crate, entries in raw_exemptions.items():
        for entry in entries:
            current: dict = {
                "crate": crate,
                "version": entry.get("version", "?"),
                "criteria": entry.get("criteria", ""),
                "notes": entry.get("notes", ""),
            }
            notes = current["notes"]
            rb_match = _REVIEW_BY_RE.search(notes)
            if rb_match:
                try:
                    current["review_by"] = date.fromisoformat(rb_match.group(1))
                except ValueError:
                    # 날짜 형식 오류 — 경고로 처리
                    current["review_by_error"] = rb_match.group(1)
            exemptions.append(current)

    return exemptions


def scan_deny_review_by(deny_path: Path) -> list[dict]:
    """deny.toml 주석 내 Review-by: 날짜를 스캔한다 (F-SC-C41-05).

    deny.toml(cargo-deny 설정)에는 [[exemptions]] 테이블이 없고 Review-by 약속이
    주석으로만 존재한다(대문자 'Review-by:'). config.toml 의 tomllib 파싱과 별개로
    원문을 case-insensitive 정규식으로 스캔해, 날짜(YYYY-MM-DD)가 있는 항목만 추출한다.
    (날짜 없는 prose 'Review-by' 약속 줄은 제외 — 만료 비교 대상이 아니다.)

    반환 항목 형태: {"crate": "deny.toml", "version": "L<lineno>", "notes", "review_by"?, "review_by_error"?}
    """
    entries: list[dict] = []
    text = deny_path.read_text(encoding="utf-8")
    for lineno, line in enumerate(text.splitlines(), start=1):
        match = _DENY_REVIEW_BY_RE.search(line)
        if not match:
            continue
        current: dict = {
            "crate": "deny.toml",
            "version": f"L{lineno}",
            "notes": line.strip(),
        }
        try:
            current["review_by"] = date.fromisoformat(match.group(1))
        except ValueError:
            current["review_by_error"] = match.group(1)
        entries.append(current)
    return entries


def assert_parse_coverage(config_path: Path, exemptions: list[dict]) -> None:
    """파싱 커버리지 invariant — fail-open 재발 차단 (F-SC-C38-01).

    파일 원문에 등장하는 'review-by' 토큰 수와, 실제로 날짜(또는 형식 오류)로
    파싱된 항목 수가 일치하는지 검증한다. 불일치 시 fail-loud(예외 발생) —
    조용히 일부만 인식하고 통과하는 fail-open 을 원천 차단한다.
    """
    raw_token_count = config_path.read_text(encoding="utf-8").count(_REVIEW_BY_TOKEN)
    parsed_count = sum(
        1 for e in exemptions if "review_by" in e or "review_by_error" in e
    )
    if parsed_count != raw_token_count:
        raise SystemExit(
            "오류: review-by 파싱 커버리지 불일치 (fail-open 위험) — "
            f"파일 원문 'review-by' 토큰 {raw_token_count}개 vs 파싱된 항목 {parsed_count}개. "
            "정규식/파서가 일부 review-by 항목을 놓치고 있습니다. "
            f"파일: {config_path}"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description="cargo-vet 면제 만료일 검증")
    parser.add_argument(
        "config_path",
        nargs="?",
        default="clients/maekon-client/supply-chain/config.toml",
        help="config.toml 경로 (기본: clients/maekon-client/supply-chain/config.toml)",
    )
    parser.add_argument(
        "--fail-on-expired",
        action="store_true",
        help="만료 항목 또는 날짜 형식 오류(malformed Review-by) 존재 시 exit 1 (기본: 경고만, exit 0). "
             "malformed 날짜는 만료 비교를 우회하므로 함께 게이트한다 (F-SC-C41 #4463).",
    )
    parser.add_argument(
        "--deny-file",
        default=None,
        help="deny.toml 경로 — 주석 내 'Review-by: YYYY-MM-DD' 약속도 만료 게이트에 합류 (F-SC-C41-05)",
    )
    args = parser.parse_args()

    config_path = Path(args.config_path)
    if not config_path.exists():
        print(f"오류: 설정 파일을 찾을 수 없음: {config_path}", file=sys.stderr)
        return 1

    exemptions = parse_exemptions(config_path)
    # F-SC-C38-01: 파싱 커버리지 invariant — fail-open 재발 차단 (불일치 시 SystemExit)
    # (deny.toml 합류 전, config.toml 토큰 수 대조이므로 config 항목만으로 검증한다.)
    assert_parse_coverage(config_path, exemptions)

    # F-SC-C41-05: deny.toml 주석의 Review-by 약속을 만료 검사에 합류 (--deny-file 지정 시).
    deny_entry_count = 0
    if args.deny_file:
        deny_path = Path(args.deny_file)
        if not deny_path.exists():
            print(f"오류: deny 파일을 찾을 수 없음: {deny_path}", file=sys.stderr)
            return 1
        deny_entries = scan_deny_review_by(deny_path)
        exemptions.extend(deny_entries)
        deny_entry_count = len(deny_entries)

    today = date.today()

    # review-by 날짜가 있는 항목만 필터
    with_date = [e for e in exemptions if "review_by" in e]
    with_date_error = [e for e in exemptions if "review_by_error" in e]
    expired = [e for e in with_date if e["review_by"] < today]
    # F-SC-C33-05: 12월 연도 롤오버 버그 수정 — timedelta(days=30) 사용
    upcoming = [e for e in with_date if today <= e["review_by"] <= today + timedelta(days=30)]

    total = len(exemptions)
    with_date_count = len(with_date)
    expired_count = len(expired)

    # F-SC-C41(#4463 G3): 게이트 실패 사유 누적 플래그.
    # 만료(expired) 뿐 아니라 날짜 형식 오류(malformed Review-by)도 --fail-on-expired 에서
    # 게이트를 실패시킨다 — malformed 날짜는 만료 비교에서 제외되어 fail-open(silent pass)
    # 되던 약점을 차단한다.
    gate_failed = False

    # F-IF-C39-03: 면제 항목이 0개인 빈 설정에서 ZeroDivisionError 방지
    # (total == 0 이면 비율을 0.0%로 표기). 빈 설정은 정상 시나리오일 수 있다.
    with_date_pct = (with_date_count / total * 100) if total else 0.0

    print(f"cargo-vet 면제 항목 검증 결과")
    print(f"  총 면제 항목: {total}")
    if deny_entry_count:
        print(f"  └ deny.toml Review-by 항목 합류: {deny_entry_count}")
    print(f"  review-by 날짜 있음: {with_date_count} ({with_date_pct:.1f}%)")
    print(f"  날짜 형식 오류: {len(with_date_error)}")
    print(f"  만료됨 (< {today}): {expired_count}")
    print(f"  30일 내 만료 예정: {len(upcoming)}")
    print()

    if with_date_error:
        print("경고: 날짜 형식 오류 항목 (만료 비교 불가 — fail-open 위험):")
        for e in with_date_error:
            print(f"  {e['crate']}@{e.get('version', '?')}: review-by:{e['review_by_error']}")
        if args.fail_on_expired:
            print(
                f"오류: {len(with_date_error)}개의 날짜 형식 오류 항목이 만료 검사를 우회합니다. "
                "유효한 'review-by: YYYY-MM-DD' 형식으로 수정 필요."
            )
            gate_failed = True
        print()

    if upcoming:
        print("주의: 30일 내 만료 예정 항목:")
        for e in upcoming:
            print(f"  {e['crate']}@{e.get('version', '?')}: {e['review_by']} — {e.get('notes', '')[:60]}")
        print()

    if expired:
        print("만료된 면제 항목:")
        for e in expired:
            print(f"  {e['crate']}@{e.get('version', '?')}: {e['review_by']} — {e.get('notes', '')[:60]}")
        print()

        if args.fail_on_expired:
            print(f"오류: {expired_count}개의 만료된 면제 항목이 존재합니다. 갱신 또는 정식 삭제 필요.")
            gate_failed = True
        else:
            print(f"경고: {expired_count}개의 만료된 면제 항목이 존재합니다. (--fail-on-expired 미지정으로 경고만 출력)")

    if with_date_count == 0:
        print("정보: review-by 날짜가 있는 면제 항목이 없습니다.")
        print("  새 면제 항목 추가 시 notes 필드에 'review-by: YYYY-MM-DD' 형식을 포함하세요.")
        print("  전체 triage: OOS-TBD-CARGO-VET-EXEMPTION-EXPIRY-TRIAGE")

    return 1 if gate_failed else 0


if __name__ == "__main__":
    sys.exit(main())
