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
  post-review:
    permissions:
      contents: read
      actions: read
      pull-requests: write
```

Common elevations: `pull-requests: write` (reviewdog **report** workflows),
`pages: write` + `id-token: write` (Pages deploy), `contents: write` (push
branches).

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

Skip this on unprivileged collect jobs that do not need git credentials.
Report workflows that fetch the PR base ref need a tokenized remote.

## Fork PR reviewdog

Public fork pull requests receive a read-only `GITHUB_TOKEN` on `pull_request`,
so reviewdog cannot post inline review comments from that event. Cadmus splits
collection from posting for every reviewdog consumer:

1. **Collect** (`pull_request`) — unprivileged. Run the linter, write
   diagnostics to a `*-reviewdog-input` artifact. No `pull-requests: write`.
2. **Report** (`workflow_run` on the collect workflow) — privileged base-repo
   context. Identify the PR, check out the PR head for the diff (see below),
   download the artifact by `run-id`, and post via reviewdog with
   `pull-requests: write`.

| Collect (`pull_request`) | Report (`workflow_run`) | Tools |
| ------------------------ | ----------------------- | ----- |
| Cargo | Clippy report | clippy |
| Actions lint | Actions lint report | actionlint, prettier |
| Shell | Shell report | shellcheck, shfmt |
| Website | Website report | prettier, eslint, stylelint |
| Docs lint | Docs lint report | rumdl |

New reviewdog jobs must follow the same collect/report pair. Keep
`pull-requests: write` on the report workflow only.

### Privileged checkout and trust

Report job order:

1. Check out the **base** repository (trusted composites under path `ci`)
2. Identify the PR (`number`, `base_ref`)
3. Check out the PR head into path `pr` and fetch the base ref
4. Download artifacts and pipe diagnostics into reviewdog (cwd `pr`)

The PR-head checkout exists solely so reviewdog can resolve `.git` and compute
the PR diff for `-filter-mode=added`. That is safe for this use case: the
privileged job must not build, install, or otherwise execute code from the fork
or from artifact payloads. Load composite actions from the base-repo `ci`
checkout only — never from the PR head. Artifacts are untrusted text only —
pipe diagnostics into reviewdog and nothing else.

`actions/checkout` v7+ refuses fork PR heads on `workflow_run` unless
`allow-unsafe-pr-checkout: true` is set. Opt in only when the checked-out
tree is never executed (data for reviewdog / `git` diff only), and keep
`persist-credentials: false`. See
https://gh.io/securely-using-pull_request_target.

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
