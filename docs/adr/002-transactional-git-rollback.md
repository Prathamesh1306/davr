# ADR-002: Session-Intersection Conflict Detection and Transactional Rollback

## Status
Accepted

## Context
When an AI agent modifies files in a codebase, developers may concurrently edit files or make edits after the agent finishes. A naive `git checkout` or `git reset` would destroy uncommitted human edits.

## Decision
1. **Intersection Rollback ($A \cap B$):** Rollback only operates on files present in the intersection of `git-diff(snapshot, HEAD)` and `session_touched_files`.
2. **Three-State Conflict Detection ($A \to B \to C$):**
   - $A$: Base snapshot state.
   - $B$: Post-session state (captured at agent exit).
   - $C$: Current workspace state.
   - If $C == B$, rollback is deterministic and safe.
   - If $C \neq B$, the file was edited by a developer post-session; it is flagged as a conflict and preserved unless `--force` is provided.
3. **Write-Ahead Rollback Journal:** Rollback transitions through `PREPARED` $\to$ `BACKED_UP` $\to$ `APPLYING` $\to$ `COMMITTED` (or `ABORTED`), backing up files before modification so partial writes can be reverted cleanly on I/O failure.

## Consequences
- Protects developer work from accidental destruction.
- Ensures recovery even if power or process failure occurs mid-rollback.
