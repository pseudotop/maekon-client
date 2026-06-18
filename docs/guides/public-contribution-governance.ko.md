# 공개 기여 거버넌스

이 문서는 Maekon Client 하이브리드 공개 기여 lane의 라벨, 소유권, 브랜치
보호 기준을 정의한다.

Maekon Client는 아직 parent source of truth에서 릴리스된다. 공개 PR은
OSS-safe 작업에 열려 있고, maintainer가 승인한 변경은 parent source tree에서
검증된 뒤 공개 저장소로 다시 export된다.

Contributor가 읽는 lifecycle summary는
[`public-contributor-path.ko.md`](./public-contributor-path.ko.md)를 따른다.

## 기여 Lane

공개 이슈나 PR을 triage할 때는 lane 라벨을 하나만 사용한다.

| Label | 대상 | 기본 처리 |
| --- | --- | --- |
| `lane:good-first-dx` | 문서, 설정 안내, 작은 테스트, 오탈자, 초보자 친화 maintenance | 공개 PR 권장 |
| `lane:local-feature` | capture, consent, egress, release 의미를 바꾸지 않는 로컬 dashboard/export/settings/UX | 일반 review 후 공개 PR 수용 가능 |
| `lane:provider-adapter` | 공개 provider metadata/spec 업데이트와 adapter 호환성 수정 | import 전 egress와 credential handling review |
| `lane:privacy-docs` | privacy 설명, consent 문구, PII 문서, 안전한 screenshot, disclosure 안내 | privacy owner review 권장 |
| `lane:trust-core` | consent, PII masking, capture, audio, automation policy, sandbox, updater, release signing, local API security | owner review와 private validation 필요 |
| `lane:enterprise-contract` | managed sync, team analytics, SSO/RBAC, admin, compliance, enterprise API contracts | 구현 전 maintainer discussion으로 routing |
| `lane:security-disclosure` | vulnerability 또는 sensitive data exposure 의심 | public issue에서 세부 논의 금지, `SECURITY.md` 사용 |

## Risk Labels

보호 surface에 영향을 줄 수 있는 변경에는 lane과 함께 다음 라벨을 추가한다.

| Label | 의미 |
| --- | --- |
| `risk:privacy` | consent, PII masking, capture, raw evidence, retention, data minimization 동작에 영향 가능 |
| `risk:security` | sandboxing, local API auth, update integrity, dependency trust, secret exposure에 영향 가능 |
| `risk:release` | packaging, signing, notarization, installer, update flow, public export에 영향 가능 |

## Hold Labels

Hold 라벨은 조건이 해소될 때까지 공개 merge 또는 parent import를 막는다.

| Label | 제거 기준 |
| --- | --- |
| `do-not-merge/security` | security owner가 public thread가 안전하고 필요한 private handling이 끝났다고 확인 |
| `do-not-merge/private-test` | maintainer가 필요한 private gate를 실행하고 공개 가능한 결과 요약을 남김 |
| `do-not-merge/needs-owner` | 관련 CODEOWNER 또는 maintainer가 현재 patch를 승인 |
| `do-not-merge/dco` | 필요한 `Signed-off-by` line 또는 legal attestation이 존재 |

## Flow Labels

| Label | 의미 |
| --- | --- |
| `ok-to-test` | maintainer가 maintainer-controlled test를 실행할 만큼 PR을 확인함 |
| `security-reviewed` | security/privacy review가 public handling path를 승인함 |
| `imported-to-parent` | 공개 변경이 release validation을 위해 parent source tree에 import됨 |

## CODEOWNERS

`.github/CODEOWNERS`는 public export에 포함되며 현재 maintainer owner를 전체
tree에 지정한다. 전용 maintainer team이 생기면 public contribution model을
바꾸지 않고 `@pseudotop`을 team으로 교체할 수 있다.

Trust-core 작업에 CODEOWNER review를 요구할 수 있도록 민감한 path를 명시한다.

- `.github/**`, release workflow, release script, update code, supply-chain metadata
- `crates/maekon-automation/**`, `crates/maekon-sandbox-worker/**`,
  `crates/maekon-vision/**`, `crates/maekon-audio/**`, `crates/maekon-network/**`,
  `crates/maekon-storage/**`
- `src-tauri/**`, `policy/**`, `api/proto/**`, `specs/providers/**`

CODEOWNER review는 trust-core 변경의 필요조건이지만 충분조건은 아니다. Private
validation이나 security review가 필요하면 위 risk/hold 라벨도 함께 사용한다.

## Branch Protection

Public `main`은 다음 설정을 가진 branch protection rule 또는 ruleset을 사용해야 한다.

1. merge 전 pull request를 요구한다.
2. merge 전 conversation resolution을 요구한다.
3. CODEOWNER review를 요구한다.
4. stale approval을 dismiss하거나 latest push approval을 요구한다.
5. fork-safe PR에서 실행되는 안정적인 public check를 required check로 지정한다.
6. `main` force push와 direct deletion을 막는다.
7. release, signing, private validation secret을 fork PR workflow에 노출하지 않는다.

Private trust-core gate가 maintainer-only validation name, raw capture,
screenshot, local path, maintainer-only infrastructure detail을 노출할 수 있다면
required public check로 표시하지 않는다. 공개 요약은 private evidence가 아니라
risk class와 안전한 결과를 설명해야 한다.

Fork-safe public matrix, maintainer-only gate trigger, workflow guardrail은
[`public-private-ci-split.ko.md`](./public-private-ci-split.ko.md)를 따른다.

수동 public PR import 절차, attribution field, export handoff comment는
[`hybrid-import-workflow.ko.md`](./hybrid-import-workflow.ko.md)를 따른다.

공개 안전 starter issue 규칙과 첫 copy-ready issue seed batch는
[`good-first-issues.ko.md`](./good-first-issues.ko.md)를 따른다.

Public-safe PR lifecycle, evidence checklist, maintainer response contract는
[`public-contributor-path.ko.md`](./public-contributor-path.ko.md)를 따른다.

## Label Sync

Maintainer는 다음 명령으로 public label set을 생성하거나 복구할 수 있다.

```bash
scripts/sync-public-contribution-labels.sh OWNER/REPO
```

공개 export 저장소가 public PR을 직접 받을 준비가 되면 같은 명령을 해당 저장소에도
실행한다.
