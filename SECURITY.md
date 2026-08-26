# Security Policy

## Overview

DAVR is a local developer tool designed to provide safety supervision, state snapshotting, and conflict-aware rollback capabilities when running autonomous AI coding agents.

This document outlines our security model, reporting guidelines, supported versions, and explicit runtime boundaries.

---

## Supported Versions

| Version | Supported | Status |
|---|---|---|
| `0.2.x` | ✅ Yes | Current Safety-Hardened Alpha |
| `0.1.x` | ❌ No | Deprecated Baseline |

---

## Reporting a Vulnerability

We take the security of DAVR and its users seriously. If you discover a security vulnerability, please report it responsibly:

1. **Do not create a public GitHub issue** for undisclosed security vulnerabilities.
2. **Report via GitHub Private Vulnerability Reporting:**  
   Navigate to the repository's **Security** tab and click **Report a vulnerability** (or open a private security advisory).
3. **Include the following information in your report:**
   - Detailed description of the vulnerability and potential impact.
   - Exact steps or minimal proof-of-concept (PoC) to reproduce the issue.
   - Affected operating system, Rust version, and DAVR release.
   - Any proposed remediation or mitigation.

### Response Timeline
- **Initial Acknowledgement:** Within 48 hours.
- **Triage & Status Update:** Within 5 business days.
- **Remediation & Advisory:** A patch will be prepared in a private fork and released alongside a public advisory acknowledging the reporter (unless anonymity is requested).

---

## Threat Model

DAVR is designed to protect local developer workstations against common failure modes introduced by AI coding agents.

### In Scope (Threats Addressed)
1. **Accidental Modification / Deletion of Developer Work:**  
   AI agents overwriting uncommitted code or deleting project files. DAVR provides Git ODB snapshots and $A \to B \to C$ 3-way conflict detection to preserve human modifications.
2. **Execution of Dangerous Top-Level Commands:**  
   Accidental execution of commands matching destructive patterns (`rm -rf /`, `git push --force`, `DROP TABLE`).
3. **Credential & Secret Exposure:**  
   Accidental leakage of high-entropy API keys (`sk-...`, `ghp_...`) into SQLite database history or telemetry logs.
4. **Path Traversal & Symlink Escape:**  
   Malicious or malformed file paths attempting to restore files outside the target workspace via `..` components or dangling symlinks.
5. **Orphaned Agent Process Groups:**  
   Agent crashes leaving background tasks running and locking workspace ports/files.

---

## Security Guarantees & Implementation

1. **Pre-Spawn Top-Level Policy Evaluation:**  
   Commands passed to `davr run` are evaluated against `SecurityConfig` rules before any process is spawned. Blocked commands return `DavrError::Security` with exit code `20`.
2. **Secret Redaction:**  
   High-entropy tokens matching configured regex patterns are scrubbed before command strings are written to the database or telemetry streams.
3. **Transactional Rollback Journal:**  
   Rollbacks stage file backups in `.davr/rollback-txn/<id>/` before applying changes. If an error occurs, modifications are rolled back to the pre-transaction state.
4. **Symlink and Path Containment:**  
   The rollback executor canonicalizes all target paths and enforces `canonical_target.starts_with(&canonical_root)`, blocking symlink escapes.
5. **Process Group Termination:**  
   Agents are spawned in dedicated Unix process groups (`setpgid(0, 0)`). Termination signals (`SIGINT` $\to$ `SIGTERM` $\to$ `SIGKILL`) are sent to the negative process group ID to cleanly terminate all child processes.

---

## Explicit Boundaries & Known Limitations

> **Crucial Distinction:**  
> DAVR is a **process supervisor and filesystem verification runtime**, **NOT** an OS-level kernel sandbox.

- **Subprocess Command Interception:**  
  DAVR strictly validates the *top-level command line* passed to `davr run -- <command>`. When an agent process spawns internal sub-processes (e.g., inside an interactive bash shell or child process fork), those sub-processes are supervised via process group lifecycle and filesystem tracking, but individual internal system calls are **not** intercepted via kernel hooks (such as seccomp, ptrace, or eBPF).
- **Network Isolation:**  
  DAVR does not restrict outbound or inbound network access for agent processes. If an agent requires network isolation, use external network namespaces or container tooling (e.g., Docker, Podman).
- **Windows Process Trees:**  
  Process group isolation currently uses Unix signals (`nix` crate). Windows Job Object containment is scheduled for a future release.
- **Concurrent Same-File Edits:**  
  If a developer and an agent modify the exact same file simultaneously, DAVR flags this as a conflict ($C \neq B$) and preserves the developer's version. Resolving the internal line-by-line diff requires manual merging or `--force`.

---

## Security Testing Approach

Security invariants in DAVR are validated via automated regression tests:
- **Matrix Integration Tests:** [`crates/davr-core/tests/safety_hardening_matrix_test.rs`](crates/davr-core/tests/safety_hardening_matrix_test.rs) validates secret redaction in persistence, top-level command blocking, and conflict handling.
- **Path Containment Unit Tests:** [`crates/davr-security/src/lib.rs`](crates/davr-security/src/lib.rs) tests symlink resolution and path boundary checks.
- **Transactional Journal Tests:** [`crates/davr-git/src/lib.rs`](crates/davr-git/src/lib.rs) tests pure 3-way conflict planning and journal recovery.
