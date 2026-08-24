# Maekon Client AGENTS.md

`clients/maekon-client/`에 적용되는 Rust/Tauri 경로 계약이다. Root `AGENTS.md`를 함께 적용한다.

## Source of truth

- 이 디렉터리가 Maekon Client의 내부 source of truth다.
- 공개 `pseudotop/maekon-client`는 검증된 snapshot export 대상이다. 공개 저장소에서 직접 개발하지
  않는다.
- workspace 구성과 crate 목록은 `Cargo.toml`의 `[workspace].members`를 현재 값으로 읽는다. 지침에
  개수를 고정하지 않는다.

## Architecture

- 이 workspace는 Hexagonal Architecture만 사용한다. 서버의 DDD 규칙을 Rust client에 투영하지
  않는다.
- `maekon-core`가 ports, models, errors를 소유한다.
- adapter crate는 `maekon-core` port를 구현하며 승인된 예외 외에 adapter끼리 직접 결합하지 않는다.
- `src-tauri/`는 composition root와 runtime orchestration을 소유한다.
- `maekon-web` handler는 transport mapping, validation, service orchestration만 수행한다.
- automation과 external integration은 policy, privacy, consent, audit gate를 유지한다.

## Language and UI

- Rust 주석, doc comment, `tracing` message는 영어로 쓴다.
- 사용자 문자열은 i18n resource로 관리하고 locale 문자열을 selector로 사용하지 않는다.
- frontend component는 shared tokens와 primitives를 사용한다.
- `supply-chain/audits.toml`의 `[[audits.*]]`·`[[trusted.*]]` notes는 영어로 쓴다. 이 파일은
  공개 저장소로 export되고 다른 조직이 `cargo vet` import로 가져간다 — 그 소비자에게 근거가
  읽혀야 한다. `supply-chain/config.toml`의 `[[exemptions.*]]` notes는 내부 판단 기록이므로
  이 제약을 받지 않는다.
- 신규 `[[audits.*]]`의 `version + safe-to-deploy`는 현재 dependency graph의 feature/target
  도달성이 아니라 crate absolute version 전체를 인증한다. 전체 target·feature를 검토한 경우에만
  notes에 `AUDIT SCOPE: full crate version; all targets and features reviewed.`를 정확히 기록한다.
  일부만 검토했다면 exemption을 유지한다(#11528).

  2026-08-19 실측: `[[audits.*]]` 576건은 이미 전부 영어였다(관례로 지켜지고 있었다).
  이 줄은 그 관례를 계약으로 옮긴 것이며 기존 항목을 소급 번역하지 않는다.

## Validation from repository root

```bash
(cd clients/maekon-client && cargo fmt --all -- --check)
(cd clients/maekon-client && cargo test -p <touched-crate>)
(cd clients/maekon-client && cargo clippy -p <touched-crate> --all-targets -- -D warnings)
```

- crate 경계를 바꾸면 `(cd clients/maekon-client && cargo metadata --no-deps)`로 workspace
  membership과 dependency direction을 확인한다.
- proto consumer contract를 바꾸면 root에서 `./scripts/sync-client-protos.sh`를 실행하고
  `./scripts/sync-client-protos.sh --check`로 readback한다.
- public export와 release evidence는 별도 release issue와 승인 절차 없이는 실행하지 않는다.

## References

- `clients/maekon-client/docs/architecture/ADR-001-rust-client-architecture-patterns.md`
- `clients/maekon-client/docs/crates/README.md`
- `.github/SSOT_PR_GOVERNANCE.md`
