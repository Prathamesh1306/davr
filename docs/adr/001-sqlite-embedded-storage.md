# ADR-001: SQLite with WAL Mode for Embedded State

## Status
Accepted

## Context
DAVR needs to persist structured metadata about agent sessions, filesystem mutations, command executions, AST symbol graphs, test results, and telemetry traces locally without requiring external services, Docker containers, or background daemon processes.

## Decision
We adopt **SQLite** with Write-Ahead Logging (`PRAGMA journal_mode=WAL`) and foreign key enforcement (`PRAGMA foreign_keys=ON`) stored inside `.davr/davr.db`.

## Consequences
- **Pros:**
  - Zero external infrastructure or setup required for developers.
  - ACID transactional consistency for rollback journaling and telemetry writes.
  - Multi-process concurrency: WAL allows concurrent readers alongside active agent writers.
  - Point-in-time online backups via SQLite `VACUUM INTO`.
- **Cons:**
  - Database file must be ignored by Git (`.gitignore`).
