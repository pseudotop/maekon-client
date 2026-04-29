# Policy Token Contract (Platform ↔ Client)

This document defines the policy-token issuance/verification contract used by automation command execution.

## Token Format

- Unsigned policy token:
  - `{policy_id}:{nonce}`
- Unsigned command-scoped policy token:
  - `{policy_id}:{nonce}:h{command_hash}`
- Signed policy token (`require_signed_token=true`):
  - `{policy_id}:{nonce}:{signature}`
- Signed command-scoped policy token (`require_signed_token=true`):
  - `{policy_id}:{nonce}:h{command_hash}:{signature}`

## Field Rules

- `policy_id`
  - MUST match an active policy in client cache.
- `nonce`
  - MUST be at least 8 characters.
  - MUST contain only `a-z`, `A-Z`, `0-9`, `_`, `-`.
  - SHOULD be generated as random UUID-like value.
- `signature` (signed policy only)
  - MUST be lowercase/uppercase hex (64 chars).
  - Computed as:
    - `sha256("{policy_id}:{nonce}:{secret}")`
    - command-scoped: `sha256("{policy_id}:{nonce}:{command_hash}:{secret}")`
- `command_hash` (optional)
  - MUST be 64-char hex.
  - MUST be prefixed with `h` in token (`h{command_hash}`).
  - Binds token to a specific command scope (`command_id`, `session_id`, `action`, `timeout_ms`).

## Signing Secret

- Environment variable:
  - `ONESHIM_POLICY_TOKEN_SIGNING_SECRET`
- Used by:
  - Platform issuer (for signed policies).
  - Client verifier (`require_signed_token=true`).

## Verification Semantics

Client validation succeeds only when all conditions are met:

1. Token format is valid.
2. Policy cache is not expired (TTL window).
3. `policy_id` maps to a cached policy.
4. `nonce` format is valid.
5. Token has not been replayed within cache TTL.
6. If policy requires signature:
   - signature field is present and valid hex.
   - signature digest matches computed value.
7. If `command_hash` is present:
   - command hash in token matches recomputed hash from command scope.

## Issuance API (Client-side Utility)

- `PolicyClient::issue_command_token(policy_id)`
  - Generates nonce.
  - Issues signed/unsigned token depending on policy.
  - Fails closed when signed policy is configured but secret is missing.
- `PolicyClient::issue_command_token_for_command(policy_id, cmd)`
  - Generates nonce.
  - Includes command scope hash in token.
  - Prevents cross-command token reuse.

## Security Notes

- Replay protection is cache-TTL bounded, so platform SHOULD use short-lived command issuance windows.
- Secret rotation SHOULD be coordinated with cache invalidation and policy refresh.
