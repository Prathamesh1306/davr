# ADR-003: Tree-sitter for Incremental and Error-Tolerant AST Parsing

## Status
Accepted

## Context
AI agents often generate code iteratively, leaving syntax partially incomplete or broken mid-session. Regular expressions are fragile, lack semantic understanding, and cannot detect syntax errors. Full compilers fail completely on syntax errors and abort.

## Decision
We adopt **Tree-sitter** with concrete language grammars (`tree-sitter-rust`, `tree-sitter-typescript`, `tree-sitter-python`, `tree-sitter-go`):
- Provides concrete syntax trees (CST) and error-tolerant parsing.
- Detects incomplete or broken syntax via `root_node().has_error()` and sets `parse_incomplete = true`.
- Successfully extracts undamaged surrounding functions, classes, and structs even when errors exist elsewhere in the file.
- Enables safe fallback in the impact engine to widen test suites when files are marked `parse_incomplete`.

## Consequences
- High-fidelity symbol extraction and cross-file reference resolution.
- Deterministic impact analysis even on broken intermediate states.
