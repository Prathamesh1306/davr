## Description

<!-- Describe the changes introduced in this PR and the problem it solves. -->

## Type of Change

- [ ] 🐛 Bug fix (non-breaking change fixing an issue)
- [ ] ✨ New feature (non-breaking change adding functionality)
- [ ] ⚠️ Breaking change (fix or feature that changes existing behavior)
- [ ] 📝 Documentation update
- [ ] ⚡ Performance optimization
- [ ] 🧪 Test suite addition / refactoring

---

## Contributor Checklist

Please check all applicable boxes before requesting a review:

### Code Quality & Standards
- [ ] My code follows the repository's style guidelines (`cargo fmt --all`).
- [ ] No warnings or errors raised by `cargo clippy --workspace --all-targets`.
- [ ] I have verified that `cargo check --workspace` passes cleanly.

### Testing & Verification
- [ ] I have added unit or integration tests for new functionality.
- [ ] All existing and new tests pass locally (`cargo test --workspace`).
- [ ] **Rollback / Git Subsystem:** If touching rollback logic, I verified 3-way conflict invariants ($A \to B \to C$) and transactional recovery in [`safety_hardening_matrix_test.rs`](crates/davr-core/tests/safety_hardening_matrix_test.rs).

### Security & Safety Considerations
- [ ] I have verified that no credentials, tokens, or personal secrets are committed.
- [ ] If adding new telemetry, logs, or database columns, secret token redaction has been applied.
- [ ] Path containment checks (`validate_path_containment`) are preserved for any filesystem mutation paths.
- [ ] Top-level security policy checks execute *before* any child process is spawned.

### Documentation
- [ ] I have updated relevant documentation in `README.md`, `SECURITY.md`, or crate docs if appropriate.
- [ ] I have added an entry to `CHANGELOG.md` under `[Unreleased]` if applicable.
