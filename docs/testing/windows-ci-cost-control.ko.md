# Windows CI 비용 통제

이 정책은 Windows 네이티브 검증을 유지하면서 비공개 저장소 CI 비용이 무제한으로
증가하지 않게 한다. 기능별 tier 계약은
[Windows 대화형 검증](./windows-interactive-validation.md)을 함께 참조한다.

## Runner 소유권

| Lane | 실행 조건 | Runner | 소유 증거 | 비용 경계 |
| --- | --- | --- | --- | --- |
| Parent 계약 게이트 | PR 및 일반 CI | 기존 Linux self-hosted runner | workflow, catalog, validator, 결정적 계약 결과 | hosted Windows를 할당하지 않음 |
| Parent Windows release lint | 주 1회 또는 검토된 수동 dispatch | GitHub-hosted `windows-latest` job 1개 | Windows 네이티브 compile/lint 및 결정적 계약 결과 | 전역 단일 동시성, 45 runner-minute 제한 |
| Parent Windows audio fixture | 주 1회 또는 검토된 수동 dispatch | GitHub-hosted `windows-latest` job 1개 | Synthetic audio/WebDriver build 및 제한된 운영자 handoff artifact | 전역 단일 동시성, 45 runner-minute 제한, 수동 실행만 upload |
| Parent per-OS patch audit | 주 1회 또는 검토된 수동 dispatch | 직렬 실행되는 `macos-latest`, `windows-latest` job | 플랫폼 전용 first-party lint | 공용 유료 concurrency, OS별 60분, 실행당 hosted runner 최대 120분 |
| 공개 export CI | 검토된 export PR/release workflow | 공개 저장소 표준 hosted Windows | release feature PE build, 서명된 VC runtime staging, ZIP/MSI/NSIS closure | 비공개 parent Windows minute를 소비하지 않음 |
| Clean-host lifecycle | 수동 dispatch만 허용 | 잠금 해제된 별도 self-hosted Windows 11 VM | 설치, 첫 실행, sidecar, UIA/toast, 재부팅, 제거, 데이터 삭제 증거 | GitHub hosted minute 과금 없음, 인프라는 운영자 소유 |

Linux runner, Wine, cross-compile 또는 Windows Server build agent는 clean
Windows 11 lifecycle 증거를 대체할 수 없다. 반대로 clean VM에서 candidate를
다시 빌드해 검토된 서명 artifact 대신 사용해서도 안 된다.

## 비공개 hosted 네이티브 runner 상한

`.github/workflows/maekon-client-windows-release-lint.yml`과
`.github/workflows/maekon-client-windows-audio-fixture.yml`은 다음 계약을
강제한다.

- `push`, `pull_request` trigger 금지
- 매일이 아닌 주 1회 자동 실행
- 유료 `windows-latest` job은 정확히 1개
- 두 workflow와 모든 ref가 하나의 전역 concurrency group 공유
- 유료 job timeout 45분
- 수동 실행은 정확한 40자 source SHA, 실행 목적,
  `confirm_paid_windows_minutes=true` 필요
- 기존 Linux self-hosted runner가 유료 Windows 할당 전에 admission 검증
- 재실행은 승인하지 않으며 운영자가 새 검토 실행을 dispatch해야 함

`.github/workflows/maekon-client-patch-audit.yml`도 macOS/Windows matrix에
최초 실행, exact SHA, 명시 확인 admission을 동일하게 적용한다. 두 leg는
`max-parallel: 1`, OS별 timeout 60분이며 나머지 두 Windows workflow와 같은
저장소 전역 유료 concurrency group을 공유한다.

이 절에서 통제하는 두 workflow의 자동 실행 합산 최악 상한은 주당 Windows
runner 90분이며, 한 달에 각 실행 요일이 5번 있는 경우 450분이다. 명시적으로
확인한 수동 실행은 실행당 최대 45분을 추가한다. 이 수치는 저장소 전체 합계가
아니며, 아래 잔여 소비자는 별도로 계산한다. GitHub 가격과 포함 quota는 변경될
수 있으므로 USD 환산값은 저장소 계약에 고정하지 않는다.

이 정책 적용 전 최근 성공 실행의 실측 wall-clock은 release lint 약 15~24분,
audio fixture 약 21~30분이었다. 45분은 cold cache 여유를 유지하면서도 실제
과금 차단선으로 동작한다.

## 잔여 비공개 Windows 소비자

의도적으로 분리한 두 소비자도 같은 계약 테스트에서 계속 감시한다.

| Workflow | 실행 조건과 상한 | 분리 유지 이유 |
| --- | --- | --- |
| `console-windows-contracts.yml` | 경로가 일치하는 PR만 실행, Windows job 1개 최대 15분 | Console의 자연스러운 Windows resolver를 검증하며 Maekon client release lane 범위 밖임 |

따라서 예약된 Maekon client Windows 작업의 정적 최악 상한은 주당 150분이다.
두 Windows workflow가 90분, patch-audit Windows leg가 60분이다. 별도 과금되는
macOS leg까지 포함한 예약 실행의 정적 hosted-runner 상한은 주당 210분이다.
Console은 경로 제한 event 기반이라 고정된 주간 합계로 표현할 수 없다.

## 운영자 체크리스트

유료 수동 실행 전:

1. 필요한 증거가 release PE 또는 package closure라면 공개 export CI를 우선한다.
2. Parent 유료 lint는 검토된 정확한 source SHA에만 사용한다.
3. 유료 workflow에서 **Re-run jobs**를 사용하지 않는다. 재실행은 admission에서
   실패하므로 새 검토 실행을 dispatch한다.
4. 동일 실행이 진행 중인지 확인한다. 전역 concurrency는 이미 과금된 실행을
   취소하지 않고 실행 중인 유료 workflow 뒤에 대기시킨다.
5. 최소 목적을 선택하고 Windows 45분 또는 patch audit hosted 120분 상한을
   명시적으로 확인한다.
6. 저장소/조직 Actions budget과 usage dashboard를 확인한다. Billing budget과
   alert는 GitHub 외부 설정이므로 YAML만으로 보장할 수 없다.

2026-08-04 현재 parent account에는 account-wide Actions 월 USD 50 budget,
**Stop usage=Yes**, included-usage 및 budget alert가 설정되어 있다. 이는 저장소별
보장이 아닌 외부 최종 차단선이며 계정의 모든 저장소 사용량이 합산되므로 매월
GitHub Billing에서 확인한다.

Clean-host 실행 전:

1. Runner 시작 전에 검토된 baseline을 복구한다.
2. 분리된 로컬 테스트 계정과 잠금 해제된 대화형 session만 사용한다.
3. Release-candidate tier가 요구하는 exact artifact URI, artifact SHA-256,
   source SHA, snapshot identity를 제공한다.
4. 수동 dispatch에서 runner 준비와 clean snapshot 복구를 모두 확인한다.
5. 증거 upload 후 runner를 중지하고 VM을 다시 baseline으로 되돌린다.

## 리뷰 규칙

Hosted Windows 또는 macOS trigger 추가, timeout 증가, matrix entry 추가, exact-SHA admission
약화 또는 두 번째 유료 job 추가 시 반드시
`scripts/ci-cost-control-contracts.test.mjs`를 갱신하고 PR 증거에 비용 변화를
기록한다. 릴리스 긴급성은 이 규칙의 예외가 아니다.
