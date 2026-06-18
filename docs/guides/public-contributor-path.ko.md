[English](./public-contributor-path.md) | [한국어](./public-contributor-path.ko.md)

# 공개 Contributor Path

이 companion guide는 Maekon Client의 공개 기여 흐름을 한곳에 정리한다. 어떤
작업이 공개 PR로 안전한지, 어떤 evidence를 남겨야 하는지, 승인된 공개 변경이
어떻게 release까지 이어지는지 설명한다.

Maekon Client는 local-first desktop software이며 privacy-sensitive runtime
surface를 가진다. OSS-safe 작업은 공개 기여를 환영하지만, 민감한 runtime 및
release 경로는 release 전에 더 강한 maintainer gate를 거친다.

## 현재 공개 계약

Maekon Client는 현재 hybrid contribution model을 사용한다.

1. Contributor가 공개 repository에 issue 또는 PR을 연다.
2. Maintainer가 하나의 contribution lane과 필요한 risk/hold label로 triage한다.
3. Fork-safe public check는 secret이나 real user data 없이 실행된다.
4. 승인된 공개 변경은 full release validation을 위해 parent source tree로
   import된다.
5. 검증된 source는 attribution과 안전한 handoff summary와 함께 public repository로
   다시 export된다.

즉 final release validation이 parent source tree에서 실행되더라도 공개 PR은 실제
product work가 될 수 있다. Maintainer는 민감한 validation evidence를 노출하지
않고 공개적으로 route를 설명해야 한다.

넓은 외부 PR intake가 공지되기 전까지는 maintainer가 public-safe로 표시한 starter
issue와 companion documentation만 게시한다.

## 작업하기 좋은 영역

가장 안전한 첫 기여는 작고 review하기 쉬우며 privacy-sensitive behavior를 바꾸지
않아도 유용한 작업이다.

| Lane | 좋은 공개 작업 | 시작점 |
| --- | --- | --- |
| Docs/DX | 설정 안내, 명령 출력 설명, 오탈자, 공개 guide 업데이트 | 범위가 좁은 issue 또는 작은 docs PR |
| i18n parity | 공개 영문/한국어 문서 정합성 유지 | 한 언어가 이미 완성된 guide |
| Synthetic examples | fake-data example, sample config, local-only playbook | fake name, fake domain, fake token을 쓰는 예시 |
| Public QA templates | 안전한 재현 문구, redaction checklist, public evidence note | 문서 전용 PR |
| Privacy documentation | masking code를 바꾸지 않고 public privacy behavior 설명 | `lane:privacy-docs` issue |

Copy-ready starter issue seed와 beginner-safe boundary는
[`good-first-issues.ko.md`](./good-first-issues.ko.md)를 참고한다.

## 구현 전에 물어볼 영역

다음에 영향을 줄 수 있다면 큰 patch를 작성하기 전에 maintainer에게 먼저 확인한다.

- consent, capture, audio, OCR, input monitoring, raw evidence handling
- PII masking behavior 또는 privacy enforcement logic
- automation policy, sandbox execution, action confirmation behavior
- external egress, provider routing, sync, telemetry
- installer, updater, signing, notarization, release automation
- local API security, dependency trust, workflow permission

이 영역도 기여를 받을 수 있지만, 구현 전에
[`public-contribution-governance.ko.md`](./public-contribution-governance.ko.md)의
lane과 review route가 필요하다.

Security vulnerability 또는 sensitive data exposure 의심은 public issue나 PR이
아니라 `SECURITY.md`의 private reporting path를 사용한다.

## PR Lifecycle

1. **Lane을 고른다.** Issue 또는 PR에는 lane label이 정확히 하나 있어야 한다.
2. **Scope를 작게 유지한다.** 작은 PR일수록 review와 import가 쉽다.
3. **Synthetic data를 사용한다.** Real customer data, private screenshot, raw
   capture, credential, local absolute path, private log를 포함하지 않는다.
4. **Public check를 실행한다.** Issue 또는 관련 public guide에 적힌 check를 사용한다.
5. **Evidence를 설명한다.** Public thread에 남겨도 안전한 command output,
   redacted screenshot, behavior note를 포함한다.
6. **Public review에 응답한다.** Maintainer가 label을 조정하거나 scope 축소,
   owner review routing을 요청할 수 있다.
7. **Import와 export를 기다린다.** 승인되면 maintainer가 patch를 parent validation
   대상으로 import하고 안전한 handoff summary를 게시한다.

Import handoff는 [`hybrid-import-workflow.ko.md`](./hybrid-import-workflow.ko.md)에
정리되어 있다. Public/maintainer-only CI 경계는
[`public-private-ci-split.ko.md`](./public-private-ci-split.ko.md)를 따른다.

## Evidence Checklist

Maintainer가 public thread를 안전하게 유지하면서 review할 수 있도록 다음 evidence를
포함한다.

- 실행한 명령과 통과 여부
- docs, UI, behavior 변경의 작은 before/after note
- 민감한 내용이 없는 redacted screenshot
- synthetic fixture name, fake domain, fake token
- 공개 issue, discussion, docs link

포함하지 않는 것:

- secret, API key, token, signing material, credential
- raw screen, audio, input, browser, OCR capture
- customer data, personal data, private log, real workspace path
- maintainer-only evidence, internal infrastructure detail, unpublished roadmap draft
- private security channel에서 다뤄야 하는 vulnerability detail

## Maintainer 응답

Maintainer는 public comment를 이해 가능하고 안전하게 유지해야 한다.

| Response | 의미 |
| --- | --- |
| `ok-to-test` | Maintainer-controlled check를 실행할 만큼 maintainer가 context를 확인함 |
| `security-reviewed` | Security/privacy review가 public handling path를 승인함 |
| `do-not-merge/needs-owner` | 책임 owner가 patch를 아직 review해야 함 |
| `do-not-merge/private-test` | Release 또는 export 전에 maintainer-only validation이 필요함 |
| `imported-to-parent` | Public patch가 full release validation을 위해 import됨 |

Maintainer-only validation이 필요할 때 public thread에는 risk class와 안전한 결과
summary만 남긴다. Raw log, screenshot, capture content, sensitive local path,
maintainer-only test detail은 남기지 않는다.

## PR을 열기 전

- [ ] 변경이 하나의 contribution lane에 속한다.
- [ ] PR이 public review에 충분히 작다.
- [ ] Example과 test는 synthetic data만 사용한다.
- [ ] Evidence는 public thread에 안전하다.
- [ ] Sensitive runtime, release, security 작업은 maintainer guidance를 받았다.
- [ ] PR description에 validation command와 필요한 경우 AI-assisted contribution
      disclosure를 포함했다.
