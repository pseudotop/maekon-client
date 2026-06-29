# 공개/비공개 CI 분리

이 문서는 Maekon Client 하이브리드 기여 lane의 CI 경계를 정의한다. 외부
contributor PR 규모가 강한 자동화를 정당화하기 전까지는 의도적으로 좁게 유지한다.

이 check를 둘러싼 contributor-facing PR route는
[`public-contributor-path.ko.md`](./public-contributor-path.ko.md)를 따른다.

## 목표

- 공개 PR 작성자는 fork-safe check에서 빠르고 실행 가능한 피드백을 받는다.
- Maintainer는 release, signing, capture, privacy, security 검증을
  maintainer-controlled gate 뒤에 둔다.
- Fork PR에는 repository secret, signing key, private validation credential,
  internal environment variable이 절대 전달되지 않는다.
- 민감한 검증은 raw evidence나 maintainer-only infrastructure detail 없이 공개
  thread에 안전하게 요약할 수 있어야 한다.

## Public Synthetic Matrix

다음 check는 source file, synthetic fixture, generated stub, public dependency
metadata, SBOM, headless browser만 사용하므로 일반 공개 pull request에서 안전하게
실행할 수 있다.

| Check | Workflow | 허용 데이터 | Phase 0/1 Required 여부 |
| --- | --- | --- | --- |
| Frontend build and E2E | `.github/workflows/ci.yml` | Synthetic frontend fixtures and Playwright artifacts | check 이름이 안정되면 Yes |
| Rust fmt/clippy/check/test | `.github/workflows/ci.yml` | Repository source and generated local stubs | check 이름이 안정되면 Yes |
| Config sync | `.github/workflows/config-sync.yml` | Static config files and generated frontend stub | Yes |
| gRPC governance | `.github/workflows/grpc-governance.yml` | Public proto files and generated code | Yes |
| Public export guardrails | `.github/workflows/ci.yml` and parent validation | Exported source tree only | `ci.yml`에서 실행되는 public CI check는 blocking; parent validation은 maintainer-controlled evidence로 유지 |
| Supply-chain and integrity checks | PR, `main` push, manual dispatch에서 `.github/workflows/security-compliance.yml` 실행 | Public dependency metadata, SBOM, generated reports | exported public supply-chain gate는 blocking |

`security-compliance.yml`은 authoritative public supply-chain gate다. 이 workflow는
RustSec audit, cargo-deny licenses/advisories/sources/bans, exemption-expiry
validation, cargo-vet, third-party notice generation, SBOM generation을 실행한다.
security-compliance check가 red이면 advisory로 취급하지 않는다.

Public synthetic check는 real screen capture, microphone input, browser session
state, OS permission dialog, signing credential, release token, external provider
credential을 요구하면 안 된다.

## Required vs Advisory Checks

Phase 0/1 안정화 이후 public branch protection은 fork-safe이고 check name이
안정적이며 contributor에게 실제로 유용한 check만 required로 지정해야 한다.

| Check class | Phase 0/1 posture | Phase 2 posture |
| --- | --- | --- |
| Config Sync / Port & Version Sync | Required | Required |
| gRPC Governance / Contract and Readiness Gate | Required | Required |
| Security & Compliance / Supply Chain Controls | Required | Required |
| CI / Rust fmt, clippy, check, test, build target | Check name이 안정되면 Required | Required |
| CI / Frontend build and E2E | Check name이 안정되면 Required | Frontend asset을 검증하는 public UI/docs surface에는 Required |
| CodeQL | 안정적인 check name으로 활성화되어 있으면 Required, 아니면 안정화 전까지 Advisory | 활성화되어 있으면 Required |
| Public CI에서 실행되는 public export guardrail | Required | Required |
| Performance gate와 budget check | Published budget과 낮은 flake rate가 없으면 Advisory | Budget, owner, failure handling이 문서화된 뒤에만 Required |
| Parent validation과 maintainer-only trust-core gate | Public required check가 아니라 label/review gate | 계속 public required check가 아님 |
| Release signing, notarization, installer provenance | Fork PR이 아니라 tag/release environment에서 Required | 동일 |

Check name이 계속 바뀌거나, 실패 메시지를 이해하려면 maintainer-only context가
필요하거나, 진단에 private data가 필요하다면 required로 승격하지 않는다. Maintainer가
private evidence 없이 public PR에서 failure와 remediation path를 설명할 수 있을 때
check를 하나씩 승격한다.

## Private Gate Triggers

Maintainer가 maintainer-controlled gate 실행 시점을 결정한다. 공개 route 설명은
`docs/guides/public-contribution-governance.md`의 라벨을 사용한다.

| Trigger | 의미 |
| --- | --- |
| `ok-to-test` | maintainer가 maintainer-controlled test를 실행할 만큼 public PR을 확인함 |
| `security-reviewed` | security/privacy review가 public handling path를 승인함 |
| `do-not-merge/private-test` | parent import 또는 release가 maintainer-only validation을 기다림 |
| `do-not-merge/security` | security handling이 진행 중이며 세부 내용을 public thread에서 논의하지 않음 |
| `imported-to-parent` | 공개 patch가 full validation을 위해 parent source tree에 import됨 |

Private gate는 real OS permission, sandbox behavior, automation policy,
installer/update flow, release signing, adversarial privacy check를 다룬다. 공개
댓글에는 다음처럼 안전한 결과만 요약한다.

> Maintainer-only privacy validation passed for the relevant risk class. No
> sensitive evidence is included in this public thread.

Public-safe trust-core report의 최소 형식은 다음이다.

- lane과 risk class
- maintainer-only validation 필요 여부
- pass/fail/blocked outcome
- 존재하는 경우 public parent PR, public export, release reference
- blocked 상태라면 짧은 remediation 또는 follow-up pointer

Private test name, private log, raw capture, screenshot, local absolute path,
maintainer-only infrastructure name, secret identifier, unpublished roadmap detail은
해당 report에 포함하지 않는다.

## Fork PR Secret Policy

Public workflow는 다음 규칙을 따라야 한다.

1. `pull_request_target`을 사용하지 않는다.
2. `pull_request`에서 실행되는 workflow는 `secrets.*`를 참조하지 않는다.
3. `pull_request`에서 실행되는 workflow는 write permission을 요청하지 않는다.
4. PR workflow는 top-level permission을 명시하고 read-only로 둔다.
5. Release, signing, deployment workflow는 `workflow_dispatch`, tag, 또는
   maintainer-controlled event에 둔다.

`scripts/ci/check-public-private-ci-split.sh` guardrail은 exported public workflow에
대해 위 1-4번 규칙을 검증한다.

## Branch Protection

Phase 0/1에서 required check는 위 표의 안정적인 public check로 제한한다.
Maintainer-only gate는 민감한 이름이나 evidence를 노출하는 public required check가
아니라 label과 공개 review summary로 표현한다.

공개 저장소가 정기적으로 외부 PR을 받기 시작하면 maintainer는 안정적인 public
check를 하나씩 required check로 추가 승격할 수 있다.

공개 patch가 parent source tree로 import된 뒤 maintainer handoff를 남길 때는
[`hybrid-import-workflow.ko.md`](./hybrid-import-workflow.ko.md)를 사용한다.
