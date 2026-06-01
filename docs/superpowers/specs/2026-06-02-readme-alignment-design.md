# README Alignment Design

**Goal:** Align README claims with current implemented behavior and document security gaps explicitly.

**Scope:** Documentation-only changes. No code modifications.

## Summary of Changes

- Update Key Features to reflect actual encryption flow, authentication defaults, file restrictions, and protocol framing.
- Add explicit warnings where README previously overstated integrity verification, session isolation, and key management.
- Align Architectural Overview steps with current behavior (fixed session id, encrypted-byte hashing, client-side retry).
- Document current authentication defaults and environment variable behavior.
- Document encryption key status (hardcoded key; helper exists but unused).

## Out of Scope

- Implementing session isolation, integrity verification of plaintext, or key management.
- Refactoring server/client logic.

## Files

- Modify: `README.md`

## Testing

- None (documentation-only).
