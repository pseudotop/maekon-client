# CLI candidate transport preparation (#12457)

This slice bounds the shared existing one-shot subprocess pipes and owns direct-child
cleanup. It does not enable Codex/Claude candidate assessment or certify an account,
billing mode, no-tools restriction or model.

## Enforced byte contracts

| Channel | Maximum accepted bytes | Overflow |
|---|---:|---|
| stdin prompt | 1,048,576 | Reject before copying; stop/reap the spawned direct child |
| stdout | 4,194,304 | Read at most limit + 1, reject and stop/reap |
| stderr | 262,144 | Independent concurrent drain with the same overflow rule |

The limits measure UTF-8/input bytes, not tokens or inference cost. The caller already
owns the input string; this helper cannot undo its earlier allocation. Output buffers
can reserve capacity above their length. This is not a total process-memory quota or
a limit on how much a CLI writes to disk.

A child can fill both output pipes before consuming stdin. Both readers, stdin
write/EOF and child wait run concurrently under one execution timer (#6262).
There are no separately detached writer/reader tasks. A supervisor owns the direct
child and observes caller cancellation after the caller's future is dropped.
Errors, timeout and cancellation drop the I/O futures, request termination and
observe child wait with at most one additional second of cleanup. If that wait
cannot finish, cleanup failure is reported; kill-on-drop remains a fallback.
A kernel-stuck process is not guaranteed to be reaped within the grace period.

The execution deadline starts when this helper is polled. A caller that resumes
after the deadline discards even a result completed earlier. This does not include
Codex schema preparation or its subsequent last-message file read. Bounded file
reading and the original Codex invocation budget are the next #12457 slice.

## Verification and boundaries

Nine transport regressions compile a standard-library-only owned Rust executable.
They exercise real pipes, EOF, exact caps, continuous-open overflow, nonconsuming
stdin, early nonzero exit, cancellation and a deliberately delayed result receiver.
Linux /proc disappearance observes direct-child reaping, including zombies.
No real user CLI, authentication command, credential, model or network request is
used. Native Windows/macOS behavior needs its own execution evidence; cross-platform
source is not installed acceptance.

Three compiled fault controls cover accepting one excess output byte, disabling
cancellation observation and delivering queued success after the original execution
deadline. Each control must fail an actual assertion, then restore the exact source.
Compilation failures do not count as behavioral detection. Test and mapped ADR gate
results belong to the current PR head's validation evidence.

The candidate schema/ID and purpose/consent gates remain separate. Existing LLM/OCR
parsers are not promoted to strict candidate JSON parsers. Authentication flags are
unchanged; the helper adds no retry or provider fallback.

Remaining #12457 acceptance includes:

- Codex output-file byte/type/UTF-8 bounds and its original invocation deadline.
- Version-specific effective no-tools/MCP/plugin restrictions and independently
  supported authentication/billing mode without extracting credentials.
- Invocation-purpose permissions, approval expiry, strict candidate schema,
  allowed IDs, duplicate/extra/trailing JSON and independent provider evaluation.
- Concurrency admission before spawning, descendant-process containment and a
  full candidate approval deadline including cleanup and every internal attempt.
- Actual-account support and quota observation before user-facing activation.

The helper duration is an execution budget, not proof of the entire candidate
authorization budget. Read-only sandbox flags do not establish no-tools behavior.
Unsupported candidate surfaces remain unsupported.
