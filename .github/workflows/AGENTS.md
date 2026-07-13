# GitHub Actions

## Workflow permissions

Set a strict default at workflow scope and elevate only in jobs that need more.
This follows [GitHub's recommended hardening](https://docs.github.com/en/actions/security-for-github-actions/security-guides/automatic-token-authentication#permissions-for-the-github_token)
and keeps new jobs safe by default.

```yaml
permissions:
  contents: read
```

Job-level `permissions` **replace** the workflow default — they do not merge.
When overriding a job, list every scope that job needs (including `contents:
read` if it still checks out code).

### Per-job elevation

Add only what a job requires:

```yaml
  actionlint:
    permissions:
      contents: read
      pull-requests: write
```

Common elevations: `pull-requests: write` (reviewdog), `pages: write` +
`id-token: write` (Pages deploy), `contents: write` (push branches).

### Rollup jobs

Rollup job names must be unique across workflows so branch protection can
require them individually (e.g. `required-cargo`, `required-docs`). These
pass/fail-only jobs should revoke token access:

```yaml
  required-cargo:
    name: required-cargo
    permissions: {}
```

Without this, they inherit the workflow `contents: read` grant unnecessarily.

### Read-only checkouts

Path-filter and validate jobs only need a read-only checkout. Prefer:

```yaml
      - uses: actions/checkout@…
        with:
          persist-credentials: false
```

Skip this on jobs that use reviewdog or other tools that rely on persisted
credentials for PR comments.

## Action pinning

Pin every third-party action to a full commit SHA with a version comment:

```yaml
uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7.0.0
```

Renovate updates both the SHA and the comment. Bare SHAs without a comment are
not tracked. Non-semver refs use the ref name as the comment (`# stable`,
`# cargo-llvm-cov`, `# latest`).

Do not add bare semver tags (`@v6`) or bare SHAs. Renovate's
`helpers:pinGitHubActionDigests` preset keeps digest pins current.

## Formatting

Lint with **rumdl** (via `treefmt` locally, `docs-lint.yml` in CI). See
`.rumdl.toml`.
