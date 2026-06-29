# 하이브리드 Import Workflow

이 문서는 승인된 공개 기여를 parent source of truth로 옮길 때 attribution,
release validation, public transparency를 잃지 않는 절차를 설명한다. Phase 0/1
에서는 기여량이 아직 낮으므로 자동화보다 감사 가능한 수동 절차를 기본으로 둔다.

Import 전 contributor-facing route는
[`public-contributor-path.ko.md`](./public-contributor-path.ko.md)를 따른다.

## 범위

다음처럼 OSS-safe이고 maintainer review 준비가 된 공개 PR에 이 workflow를 사용한다.

- 문서와 developer experience 수정
- i18n parity와 copy consistency
- synthetic data만 사용하는 예제
- capture, consent, egress, automation policy, sandbox, updater, release
  signing 의미를 바꾸지 않는 local UI 또는 local export 변경
- private validation detail을 드러내지 않는 public QA template

Security report나 먼저 private handling이 필요한 민감한 runtime 변경에는 이
workflow를 사용하지 않는다. 그런 변경은
[`public-contribution-governance.ko.md`](./public-contribution-governance.ko.md)의
label과 hold state를 따른다.

## Import 결정

Phase 0/1의 patch import는 수동으로 유지한다. Hybrid lane에서 낮은 위험의 공개
PR을 최소 5건 이상 처리하고 attribution, parent validation, public handoff
comment가 안정적으로 확인되기 전까지 patch import를 자동화하지 않는다.

Import의 기본 legal posture는 DCO다. 일반적인 낮은 위험의 public PR에는 CLA를
요구하지 않는다. Corporate-sponsored, patent-sensitive, 또는 비표준 IP/licensing
contribution은
[`public-contribution-governance.ko.md`](./public-contribution-governance.ko.md)에
정의된 대로 import 전에 maintainer legal review로 routing한다.

수동 import를 기본으로 두는 이유는 maintainer가 다음을 직접 확인할 수 있기
때문이다.

- 공개 patch에 secret, private screenshot, raw capture content, local absolute
  path, private validation name이 없는지
- contribution lane과 risk label이 맞는지
- DCO 또는 다른 legal attestation 기대치가 충족되는지
- parent source tree가 public PR link와 author attribution을 보존한 patch를
  받는지
- public repository를 다시 생성하기 전에 release/export validation이 실행되는지

## 수동 절차

1. 공개 PR에 lane label을 정확히 하나 붙이고 필요한 risk/hold label을 추가한다.
2. 공개 PR이 synthetic data를 사용하고 privacy-safe evidence를 포함하는지 확인한다.
3. Maintainer review가 끝났고, DCO/legal posture가 명확하며, 미해결 public review
   thread가 없는지 확인한다.

   DCO 또는 CLA required status check가 없다면 import 전에 public branch를
   수동으로 확인한다.

   ```bash
   git log --format=%B <public-base>..<public-pr-head> | grep -Eq '^Signed-off-by: .+ <[^>]+>$'
   ```

   Signed-off-by가 없다면 maintainer-approved legal attestation link를 parent PR에
   기록한다. 그 전에는 `do-not-merge/dco`를 해제하지 않는다.

4. Import 전용 parent-source branch를 만든다.
5. 공개 PR의 patch를 import한다. Commit이 깨끗하고 scope가 작으면 원본 commit을
   보존하고, 그렇지 않으면 수동 squash 후 parent commit body에 author attribution을
   남긴다.
6. Parent commit 또는 parent PR에 attribution metadata를 추가한다.

   ```text
   Public-PR: https://github.com/<public-owner>/<public-repo>/pull/<number>
   Public-Issue: https://github.com/<public-owner>/<public-repo>/issues/<number>
   Original-Author: <name or handle>
   Co-authored-by: <name> <email>
   Signed-off-by: <name> <email>
   ```

7. Lane과 risk class에 맞는 parent validation을 실행한다.
8. 검증된 parent source에서 public export를 다시 생성한다.
9. Public PR에 safe handoff comment를 남기고 공개 저장소가 해당 label을 쓸 준비가
   되면 `imported-to-parent`를 적용한다.
10. Public repository policy에 따라 public PR을 close 또는 merge한다.

## Export Handoff

Parent validation이 통과한 뒤 public handoff에는 공개해도 안전한 요약만 포함한다.

- parent import 상태
- public export 또는 release reference
- 통과한 public check
- private validation 필요 여부와 공개 가능한 safe outcome summary
- 남은 공개 follow-up 작업

Private validation이 있었을 때는 다음 문체를 사용한다.

> Parent source tree로 import했고 해당 risk class에 맞게 검증했습니다.
> Maintainer-only validation은 통과했으며, 민감한 evidence는 이 공개 thread에
> 포함하지 않습니다. 검증된 변경은 다음 public export 또는 release reference에
> 반영됩니다.

Private log, screenshot, raw capture, private test name, maintainer-only
infrastructure detail, local absolute path는 공개 comment에 게시하지 않는다.

## 자동화 Trigger

실제 데이터가 충분히 쌓인 뒤 잘못된 workflow를 고정하지 않을 만큼 안정적일 때만
scripted import helper를 검토한다. Helper는 안전한 기계적 작업으로 제한한다.

- 공개 PR patch fetch
- public PR URL과 author metadata 존재 확인
- parent import branch 생성
- conflict를 조용히 해결하지 않는 patch 적용
- attribution field를 포함한 commit message template 준비
- public export guardrail 실행

Helper는 private validation 실행, CODEOWNER review 우회, public comment 자동 게시,
fork-controlled code에 maintainer credential 노출을 해서는 안 된다.

다음 조건을 모두 만족하기 전에는 manual import에서 scripted import로 이동하지 않는다.

- 낮은 위험의 public PR 최소 5건이 attribution correction 없이 import, parent
  validation, export, public handoff를 완료함
- [`public-private-ci-split.ko.md`](./public-private-ci-split.ko.md)에 따라 required
  public check set이 안정됨
- Maintainer dry-run 2회에서 helper가 manual attribution field를 재현하고 conflict
  발생 시 조용히 진행하지 않고 중단함
- Precondition 실패 시 parent source tree와 public PR을 변경하지 않는 rollback path가
  helper 문서에 있음
