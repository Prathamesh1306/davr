# ADR-004: Model Context Protocol (MCP) for Agent Integration

## Status
Accepted

## Context
AI developer assistants and IDE extensions (Cursor, VS Code, Claude Desktop) need direct, programmatic access to DAVR's verification capabilities (running tests, querying doctor status, performing impact analysis, taking snapshots) without screen-scraping CLI stdout.

## Decision
We implement a JSON-RPC 2.0 stdio server conforming to the **Model Context Protocol (MCP)** in `davr-mcp` (accessible via `davr mcp`):
- Implements MCP schema tools: `davr_doctor`, `davr_status`, `davr_run_tests`, `davr_analyze_impact`, `davr_create_snapshot`, `davr_rollback`.
- Enforces strict read-only / mutation guardrails via `[mcp] allow_mutating_tools` config.

## Consequences
- Native integration with modern AI developer tooling.
- Structured, machine-readable verification responses.
