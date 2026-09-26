# DAVR GitHub Action (`davr-action`)

> Deterministic verification, AST change impact analysis, and flaky test detection for AI coding agents in CI.

This GitHub Action integrates the [DAVR](https://github.com/davr-dev/davr) runtime into your GitHub Actions workflow, verifying code changes on pull requests with impact-driven testing and automated sticky PR comments.

---

## 🚀 Quick Start

Add `.github/workflows/verify.yml` to your repository:

```yaml
name: Agent Verification

on:
  pull_request:
    branches: [main]

permissions:
  contents: read
  pull-requests: write

jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - name: Checkout Code
        uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Run DAVR Verification
        uses: Prathamesh1306/davr/action@main
        with:
          github-token: ${{ secrets.GITHUB_TOKEN }}
          post-pr-comment: 'true'
          fail-on-flaky: 'false'
```

---

## ⚙️ Inputs

| Input | Description | Default |
| :--- | :--- | :--- |
| `davr-version` | Pinned version of the DAVR runtime | `latest` |
| `config-path` | Path to CI-controlled DAVR config file | `""` |
| `base-ref` | Target branch for change impact analysis | `""` |
| `fail-on-flaky` | Whether to fail CI if flaky tests are detected | `false` |
| `post-pr-comment`| Post / update sticky report comment on PR | `true` |
| `test-frameworks`| Override detected test frameworks (`cargo_test`, `pytest`, `jest`, `go_test`) | `""` |
| `working-directory` | Subdirectory to run DAVR in | `.` |
| `github-token` | GitHub token for posting PR comments | `${{ github.token }}` |

---

## 📤 Outputs

| Output | Description |
| :--- | :--- |
| `verification-status` | Overall verification outcome: `passed` or `failed` |
| `impacted-file-count` | Number of source files impacted by changes |
| `impacted-test-count` | Number of tests impacted and run |
| `flaky-test-count` | Count of flaky or unstable tests detected |
| `report-json-path` | Path to full JSON verification report artifact |

---

## 🔒 Security & Poisoned Configuration Prevention

Per PRD Part 5 §9.2, CI workflows can supply their own verified config via `config-path` rather than relying on untrusted configurations committed inside PR branches.
