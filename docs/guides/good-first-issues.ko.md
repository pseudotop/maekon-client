[English](./good-first-issues.md) | [한국어](./good-first-issues.ko.md)

# Good First Issues

이 문서는 Maekon Client에 처음 기여하는 사람이 공개해도 안전한 작은 작업을
고를 수 있도록 돕는 on-ramp이다. 어떤 starter issue가 의도적으로 작고, 어떤
영역은 초보자 작업에서 제외되는지, 첫 PR에 어떤 evidence가 필요한지 설명한다.

함께 볼 문서: [CONTRIBUTING.md](../../CONTRIBUTING.md),
[공개 contributor path](./public-contributor-path.ko.md),
[공개 기여 거버넌스](./public-contribution-governance.ko.md),
[public/private CI split](./public-private-ci-split.ko.md).

## 공개 안전 Starter 규칙

`good first issue`는 trust-core 동작을 건드리지 않고도 유용해야 한다. 공개
review가 가능해야 하고, synthetic data만 사용해야 하며, contributor가 private
credential이나 private test artifact 없이 실행할 수 있는 검증 경로가 있어야 한다.

좋은 starter issue는 보통 다음 lane 중 하나에 속한다.

| Lane | 좋은 starter 예시 | 기본 라벨 |
| --- | --- | --- |
| Docs/DX | 설정 안내, 오탈자, 명령 출력 설명 개선 | `good first issue`, `lane:good-first-dx` |
| i18n parity | 공개 가이드 번역 또는 영문/한국어 공개 문구 동기화 | `good first issue`, `lane:good-first-dx` |
| Synthetic examples | 가짜 이름, 가짜 도메인, 가짜 token을 쓰는 예시 추가 | `good first issue`, `lane:good-first-dx` |
| Public QA templates | redaction checklist, public evidence 문구, 재현 정보 개선 | `good first issue`, `lane:good-first-dx` |
| Privacy documentation | masking 동작을 바꾸지 않는 공개 privacy 안내 보강 | `good first issue`, `lane:privacy-docs`, `risk:privacy` |

## Good-First가 아닌 영역

다음 surface에는 변경이 작아 보여도 `good first issue`를 붙이지 않는다. Owner
review가 필요하고 maintainer-only validation이 필요할 수 있다.

| Surface | 초보자용으로 안전하지 않은 이유 |
| --- | --- |
| 화면, 오디오, OCR, 입력 capture 동작 | consent, raw evidence, privacy promise에 영향 가능 |
| PII masking 구현 또는 sanitizer regression test | trust-core privacy 동작 변경 가능 |
| Automation policy, sandbox worker, action execution | 로컬 safety boundary에 영향 가능 |
| External egress, AI provider routing, sync, telemetry | local trust boundary 밖으로 데이터 노출 가능 |
| Updater, installer, signing, notarization, release workflow | supply-chain 및 release integrity에 영향 가능 |
| Private CI, maintainer-only test catalog, maintainer-only evidence | 내부 검증 세부 내용 유출 가능 |
| Fork workflow secret 또는 GitHub Actions permission | untrusted code에 credential 노출 가능 |

이 surface를 건드리면 beginner-friendly로 표시하지 말고
[공개 기여 거버넌스](./public-contribution-governance.ko.md)의 라벨을 사용한다.

## 첫 PR 흐름

1. Lane label이 정확히 하나 붙은 starter issue를 고른다.
2. 편집 전에 연결된 가이드 또는 파일을 읽는다.
3. 변경 범위를 작고 공개 안전하게 유지한다.
4. Synthetic example만 사용한다. 실제 customer data, private screenshot, raw
   capture text, raw audio/input data, credential, local absolute path, private
   log를 포함하지 않는다.
5. 이슈에 적힌 가장 작은 관련 check를 실행한다.
6. PR에는 issue 번호, 실행한 명령, privacy-safe evidence를 적는다.

Docs-only 변경에는 보통 다음 check가 유용하다.

```bash
git diff --check
./scripts/check-language.sh i18n
```

Trust-core가 아닌 Rust source 변경에는 maintainer가 다음을 요청할 수 있다.

```bash
cargo fmt --check
cargo test -p <crate>
```

Fork PR에서 maintainer-only private gate를 실행하거나 요청하지 않는다. 필요할 때
maintainer가 공개 가능한 결과 요약을 남긴다.

## Starter Issue Batch

아래 seed는 첫 공개 안전 issue batch로 복사해서 사용할 수 있다. 실제 외부 PR
intake가 준비된 뒤에만 공개 저장소에 게시한다.

### GFI-DOC-01: Fresh Checkout Setup 명확화

Labels: `good first issue`, `lane:good-first-dx`

Scope:

- `docs/testing/source-build-prerequisites.md` 또는 `docs/install.md`의 문구를
  개선한다.
- 헷갈리는 설정 단계 하나를 공개 명령만으로 설명한다.
- 변경은 문서에만 한정한다.

Validation:

- `git diff --check`
- 변경한 상대 링크 수동 확인

Out of scope:

- Release signing, installer behavior, updater behavior, private build script,
  local absolute path.

### GFI-I18N-01: 공개 가이드 한국어 Companion 추가

Labels: `good first issue`, `lane:good-first-dx`

Scope:

- 영문 원문이 있는 공개 가이드의 한국어 companion을 추가하거나 갱신한다.
- heading과 link target을 영문 문서와 맞춘다.
- Product identifier, command name, log key, file path는 English로 유지한다.

Validation:

- `git diff --check`
- `./scripts/check-language.sh i18n`

Out of scope:

- Internal planning, private review, roadmap, maintainer-only test file 번역.

### GFI-EXAMPLE-01: Synthetic Automation Playbook 예시 추가

Labels: `good first issue`, `lane:good-first-dx`

Scope:

- `docs/guides/automation-playbook-templates.md`에 작은 예시 하나를 추가한다.
- 가짜 service, 가짜 domain, 가짜 user name만 사용한다.
- Managed cloud sync를 약속하지 않고 local-only 기대 동작을 설명한다.

Validation:

- `git diff --check`
- 실제 credential이나 private data가 없는지 수동 확인

Out of scope:

- Automation policy execution, sandbox worker behavior, egress rule, runtime
  permission 변경.

### GFI-QA-01: Public QA Evidence 문구 개선

Labels: `good first issue`, `lane:good-first-dx`

Scope:

- `docs/qa/README.md` 또는 공개 QA checklist의 문구를 개선한다.
- Redaction과 reproduction 기대치를 더 명확하게 만든다.
- 예시는 public PR comment에 안전한 synthetic data만 사용한다.

Validation:

- `git diff --check`
- Private screenshot, raw capture, private log, private test name이 없는지 수동 확인

Out of scope:

- Maintainer-only test 이름, raw maintainer evidence, maintainer-only artifact path 추가.

### GFI-PII-DOC-01: Synthetic Privacy 문서 예시 추가

Labels: `good first issue`, `lane:privacy-docs`, `risk:privacy`

Scope:

- `docs/guides/pii-sanitization-contract.md`에 문서 전용 before/after 예시를 추가한다.
- `user@example.test`, `sk-test-redacted`, `/Users/example/project` 같은 가짜 값을
  사용한다.
- Sanitizer code를 바꾸지 않고 기대 marker를 설명한다.

Validation:

- `git diff --check`
- 예시가 synthetic이고 private test 내부를 설명하지 않는지 수동 확인

Out of scope:

- `crates/maekon-vision/**`, `src-tauri/**`, sanitizer behavior, sanitizer
  regression test 편집. Maintainer가 별도 재분류하지 않는 한 trust-core이다.

## Maintainer Triage Checklist

Starter issue를 공개하기 전에 확인한다.

- [ ] Lane label이 정확히 하나다.
- [ ] 위 trust-core surface를 건드리지 않는다.
- [ ] Public code, public docs, synthetic data만으로 완료할 수 있다.
- [ ] 편집할 가장 작은 파일 범위를 명시한다.
- [ ] Public check만 적는다.
- [ ] Secret, private screenshot, raw capture, private log, customer data, local
      absolute path를 포함하지 말라고 안내한다.
- [ ] Non-public plan, maintainer-only test catalog, internal release evidence 접근을 요구하지 않는다.

## 도움 받기

범위가 starter 설명보다 넓어 보이면 구현 전에 discussion을 열거나 issue에 댓글을
남긴다. 확신이 없을 때는 PR을 더 작게 유지하고 maintainer에게 다른 lane으로
옮겨야 하는지 확인한다.
