# Windows CI 비용 통제

이 정책은 Windows 네이티브 검증을 유지하면서 비공개 저장소 CI 비용이 무제한으로
증가하지 않게 한다. 기능별 tier 계약은
[Windows 대화형 검증](./windows-interactive-validation.md)을 함께 참조한다.

## Runner 소유권

| Lane | 실행 조건 | Runner | 소유 증거 | 비용 경계 |
| --- | --- | --- | --- | --- |
| Parent 계약 게이트 | PR 및 일반 CI | 기존 Linux self-hosted runner | workflow, catalog, validator, 결정적 계약 결과 | hosted Windows를 할당하지 않음 |
| Parent Windows release lint | 검토된 release-candidate 수동 dispatch만 허용 | GitHub-hosted `windows-latest` job 1개 | Windows 네이티브 compile/lint 및 결정적 계약 결과 | 전역 단일 동시성, 45 runner-minute 제한 |
| Parent E19 desktop smoke | 검토된 release smoke 수동 dispatch만 허용 | GitHub-hosted `windows-latest` job 1개 | Exact-SHA `automation.gui.benchmark_report.v1` 릴리스 증거 | 공용 유료 concurrency, 90 runner-minute 제한, 자동 trigger 없음 |
| Parent Windows audio fixture | 주 1회 또는 검토된 수동 dispatch | GitHub-hosted `windows-latest` job 1개 | Synthetic audio/WebDriver build 및 제한된 운영자 handoff artifact | 전역 단일 동시성, 45 runner-minute 제한, 수동 실행만 upload |
| Parent per-OS patch audit | 주 1회 또는 검토된 수동 dispatch | 직렬 실행되는 `macos-latest`, `windows-latest` job | 플랫폼 전용 first-party lint | 공용 유료 concurrency, OS별 60분, 실행당 hosted runner 최대 120분 |
| 공개 export CI | 검토된 export PR/release workflow | 공개 저장소 표준 hosted Windows | release feature PE build, 서명된 VC runtime staging, ZIP/MSI/NSIS closure | 비공개 parent Windows minute를 소비하지 않음 |
| Consumer Windows lifecycle | 비공개 실행 lane 없음 | 없음 | consumer Windows 11, unlocked-desktop UIA, 재부팅, 제거 및 데이터 삭제는 명시적으로 미검증 | 향후 전용 runner 결정은 새 검토 이슈 필요 |

Linux runner, Wine, cross-compile 또는 GitHub-hosted Windows Server build
agent는 consumer Windows 11 lifecycle 증거를 대체할 수 없다. Release
manifest는 hosted compile/lint 결과에서 lifecycle 증거를 추론하지 말고 이
경계를 기록해야 한다. 공개 저장소 hosted runner 범위는 이 비공개 parent
정책으로 변경하지 않는다.

## 비공개 hosted 네이티브 runner 상한

`.github/workflows/maekon-client-windows-release-lint.yml`과
`.github/workflows/maekon-client-desktop-smoke.yml`은 다음 release lane 계약을
강제한다.

- `push`, `pull_request` trigger 금지
- 자동 schedule 없음
- workflow별 유료 `windows-latest` job은 정확히 1개
- 저장소 전역 유료 concurrency group 공유 및 cancellation 비활성화
- release lint timeout 45분, E19 smoke timeout 90분
- release lint 수동 실행 목적은 `release-candidate`로 고정하며 정확한 40자 source SHA와
  `confirm_paid_windows_minutes=true` 필요
- release lint Linux admission job이 유료 Windows 할당 전에 입력 검증
- 재실행은 승인하지 않으며 운영자가 새 검토 실행을 dispatch해야 함

E19 lane은 수동 dispatch된 정확한 40자 commit SHA만 받고
`confirm_paid_windows_minutes=true`를 요구하며 rerun을 거부한다. Release tag
gate가 요구하는 GUI benchmark receipt를 생성하지만, 퇴역한 consumer Windows
lifecycle lane을 복구하거나 clean VM, 재부팅, 제거, 데이터 삭제 증거를 주장하지
않는다.

`.github/workflows/maekon-client-patch-audit.yml`도 macOS/Windows matrix에
최초 실행, exact SHA, 명시 확인 admission을 동일하게 적용한다. 두 leg는
`max-parallel: 1`, OS별 timeout 60분이며 나머지 두 Windows workflow와 같은
저장소 전역 유료 concurrency group을 공유한다.

Release lane의 자동 실행 상한은 0분이다. 명시적으로 확인한 lint dispatch는
실행당 최대 Windows runner 45분, E19 release smoke는 최대 90분을 소비한다. 이
수치는 저장소 전체 합계가 아니며, 아래 잔여 소비자는 별도로 계산한다. GitHub
가격과 포함 quota는 변경될 수 있으므로 USD 환산값은 저장소 계약에 고정하지 않는다.

이 정책 적용 전 최근 성공 실행의 실측 wall-clock은 release lint 약 15~24분,
audio fixture 약 21~30분이었다. 45분은 cold cache 여유를 유지하면서도 실제
과금 차단선으로 동작한다.

## 잔여 비공개 Windows 소비자

의도적으로 분리한 소비자도 같은 계약 테스트에서 계속 감시한다.

| Workflow | 실행 조건과 상한 | 분리 유지 이유 |
| --- | --- | --- |
| `console-windows-contracts.yml` | 경로가 일치하는 PR만 실행, Windows job 1개 최대 15분 | Console의 자연스러운 Windows resolver를 검증하며 Maekon client release lane 범위 밖임 |

따라서 예약된 Maekon client Windows 작업의 정적 최악 상한은 주당 105분이다.
Audio fixture 45분과 patch-audit Windows leg 60분이다. 별도 과금되는 macOS
leg까지 포함한 예약 실행의 정적 hosted-runner 상한은 주당 165분이다.
Console은 경로 제한 event 기반이라 고정된 주간 합계로 표현할 수 없다.

## 운영자 체크리스트

유료 수동 실행 전:

1. 필요한 증거가 release PE 또는 package closure라면 공개 export CI를 우선한다.
2. Parent 유료 lint와 E19 release smoke는 검토된 정확한 source SHA에만 사용한다.
3. 유료 workflow에서 **Re-run jobs**를 사용하지 않는다. 재실행은 admission에서
   실패하므로 새 검토 실행을 dispatch한다.
4. 동일 실행이 진행 중인지 확인한다. 전역 concurrency는 이미 과금된 실행을
   취소하지 않고 실행 중인 유료 workflow 뒤에 대기시킨다.
5. 최소 목적을 선택하고 lint 45분, E19 smoke 90분 또는 patch audit hosted
   120분 상한을 확인한다.
6. 저장소/조직 Actions budget과 usage dashboard를 확인한다. Billing budget과
   alert는 GitHub 외부 설정이므로 YAML만으로 보장할 수 없다.

2026-08-04 현재 parent account에는 account-wide Actions 월 USD 50 budget,
**Stop usage=Yes**, included-usage 및 budget alert가 설정되어 있다. 이는 저장소별
보장이 아닌 외부 최종 차단선이며 계정의 모든 저장소 사용량이 합산되므로 매월
GitHub Billing에서 확인한다.

퇴역한 비공개 interactive workflow를 우회 경로로 다시 만들지 않는다. 향후
consumer Windows lifecycle lane은 새 검토 이슈가 필요하며 archived interactive
validation 문서의 격리, snapshot, 증거 경계 요구를 복구해야 한다.

## 리뷰 규칙

Hosted Windows 또는 macOS trigger 추가, timeout 증가, matrix entry 추가, exact-SHA admission
약화 또는 두 번째 유료 job 추가 시 반드시
`scripts/ci-cost-control-contracts.test.mjs`를 갱신하고 PR 증거에 비용 변화를
기록한다. 릴리스 긴급성은 이 규칙의 예외가 아니다.
