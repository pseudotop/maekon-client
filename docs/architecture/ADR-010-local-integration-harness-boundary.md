# ADR-010: Local Integration Harness Is A Compatibility Harness, Not A Contract Authority

Date: 2026-03-17
Status: Accepted

## Context

The client integration domain is now mature enough to support stronger end-to-end compatibility tests.

This repository benefits from a local fake integration server because it lets the client validate:

- bootstrap and session setup
- HTTP long-poll delivery semantics
- egress acknowledgement behavior
- inbox prompt pull behavior
- basic heartbeat and disconnect behavior

An upstream project still exists and may own or co-own the real remote integration contract.

That means a local fake server is useful, but it must not become the accidental source of truth for server-side semantics.

## Decision

This repository may host a local fake integration server for client-side compatibility testing.

The harness is explicitly allowed under these rules:

1. It is a consumer-side compatibility harness only.
2. It does not define cross-repository contract truth by itself.
3. It must stay derived from the current client-facing integration contracts.
4. It must be used to validate client behavior, not to unilaterally freeze remote semantics.

Current location:

- `crates/oneshim-network/tests/fake_integration_server/`

## Rationale

The harness gives immediate value:

- catches client regressions in bootstrap/session/egress/inbox flows
- keeps transport behavior reviewable
- allows compatibility scenarios without depending on an always-running remote project

At the same time, a local fake server has a clear risk:

- tests can pass against a protocol shape that the upstream service never intended to support

The correct balance is to allow the harness, but classify it correctly.

## Consequences

### Positive

- the client repo can run realistic compatibility tests locally and in CI
- transport and envelope regressions are easier to catch early
- integration runtime hardening can proceed without waiting for a full remote environment

### Negative

- the harness still needs periodic review against upstream contract evolution
- passing local compatibility tests is not sufficient proof of cross-project interoperability

## Operational Rule

Use the local fake server to validate:

- client runtime behavior
- serialization and parsing behavior
- reconnect, ack, inbox, and lifecycle handling

Do not use the local fake server alone to claim:

- upstream server parity
- final transport semantics
- final auth/bootstrap semantics across repositories

## Follow-Up

1. Expand the harness scenario suite gradually
   - reconnect
   - partial ack
   - duplicate delivery
   - prompt receipt
   - retry and `Retry-After`
   - auth reset and recovery

2. Review harness scenarios against the upstream project when broader end-to-end validation is needed

3. Keep AsyncAPI and CloudEvents profile documents aligned with the harness and the runtime

## Related

- Internal integration-domain implementation plan and architecture design notes
- `docs/contracts/integration-asyncapi.yaml`
- `docs/contracts/integration-cloudevents-profile.md`
