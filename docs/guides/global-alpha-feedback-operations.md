[English](./global-alpha-feedback-operations.md) | [한국어](./global-alpha-feedback-operations.ko.md)

# Global Alpha Feedback, Privacy, and Incident Operations

This guide is the operational contract for invited Maekon Global Alpha
feedback. The machine-readable source of truth is
`docs/contracts/global-alpha-feedback-policy.json`; the canonical public notice and
email-draft form is `https://maekon.dev/alpha-feedback`.

## Current gate

Policy `2026-07-19.1` is in `hold`. New invitations, general Alpha intake, and
scheduled Alpha posts are not allowed while repository issue #8683 remains on
HOLD. Privacy requests, participation withdrawal, and private vulnerability
reporting remain open. Changing `intake_state` requires a reviewed manifest
change and cannot bypass the repository release gate.

## Route separation

| Request | Route | Public issue allowed |
|---|---|---|
| Invited Alpha feedback | `support@maekon.dev`, subject `[Maekon Alpha Feedback]` | No participant or diagnostic data in a public issue |
| Privacy/access/correction/deletion | `support@maekon.dev`, subject `[Maekon Alpha Privacy Request]` | No |
| Withdraw participation | `support@maekon.dev`, subject `[Maekon Alpha Withdrawal]` | No |
| Security vulnerability | GitHub private vulnerability report | Never |

The public page sends no network request and stores no form value. It constructs
a local `mailto:` draft so the sender can review it. Opening or sending that
draft is not a receipt; the support reply described below is the receipt.

## Intake boundary

Accepted fields are request type, contact email, acquisition source, OS, exact
version or commit, a short synthetic summary, participation consent, separate
quote consent, diagnostic attachment opt-in, policy version, and UTC submission
time.

Do not request or accept:

- raw screens, screenshots, OCR text, or window titles;
- prompts, conversation content, secrets, credentials, or customer data;
- another person's email or any full local path;
- an automatically uploaded diagnostic bundle.

The page has no file input. If diagnostics are necessary, the participant must
generate the bundle from Maekon **Support and Diagnostics**, review the exported
content, and attach it manually. Diagnostic opt-in does not imply quote,
research, product, or telemetry consent.

## Receipt and service levels

The operator replies from the support mailbox within three business days using
this minimum receipt:

```text
MAEKON-ALPHA-RECEIPT
receipt_id: <opaque id>
received_at: <UTC timestamp>
request_type: <feedback|privacy|withdrawal>
policy_version: 2026-07-19.1
target_by: <UTC date>
```

Privacy or withdrawal completion targets 30 calendar days after verified
receipt. A critical install, crash-loop, data-loss, or privacy-boundary incident
is triaged within 24 hours. These are operating targets, not proof that a
particular request was received or completed.

## Storage and access

- Data owner and only listed mailbox operator: `pseudotop`.
- General feedback retention: at most 90 days.
- Closed contact record and opted-in diagnostic attachment retention: at most
  30 days.
- A verified earlier deletion request overrides those maximums.
- User content, direct identifiers, mail bodies, and diagnostic attachments must
  not be copied into GitHub issues, Project fields, outreach logs, or D7
  readbacks.

Only privacy-safe aggregate counts may cross into #8688 or #8697. Counts must
retain their denominator, observation window, and query/export receipt.

## Daily triage

The `pseudotop` operator performs one daily pass while intake is active:

1. Classify each new message as feedback, privacy, withdrawal, or security.
2. Redirect security reports to the private vulnerability route without copying
   exploit detail into a public surface.
3. Send the timestamped receipt and assign the applicable target date.
4. Verify that attachments were explicitly opted in and reviewed; reject and
   delete unexpected raw content.
5. Record only an opaque receipt ID, status, policy version, and severity in the
   private operator register.
6. Update #8688 with aggregate outreach state and hand D7 aggregates to #8697;
   never post participant-level content.

## Withdrawal and deletion

After a verified withdrawal, stop future Alpha contact and participant-level
measurement immediately, remove direct identifiers and retained attachments,
then send a completion reply. A previously computed, non-reidentifiable
aggregate count may remain; it cannot be used to resume contact, recreate the
participant record, or claim retention/customer evidence.

Product-local Maekon data and Alpha contact records are separate. Withdrawing
from Alpha does not itself delete local device data; the participant must use
Maekon's local deletion control for that scope. Conversely, local deletion does
not prove that a separately sent support email has been deleted.

## Fail-closed incident pause

On a confirmed critical install failure, crash loop, data loss, or privacy
boundary mismatch:

1. keep privacy, withdrawal, and private security channels open;
2. stop new invitations and scheduled posts;
3. set the policy manifest to `paused` in a reviewed change;
4. record the incident and owner on #8683 without participant content;
5. require a new release decision before reopening general intake.

Feedback, interviews, diagnostics, registrations, and receipts are not customer,
revenue, retention, product-value, or stable-release evidence.
