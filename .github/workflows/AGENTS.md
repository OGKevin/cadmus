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

## Fork PRs and secrets

`github.event.repository.fork` is true only when the workflow runs **on a fork
repository** (a push to that fork, or a pull request opened inside it). A
`pull_request` into the upstream repository still has `repository.fork == false`,
and GitHub withholds repository secrets and write scopes on that event.

Jobs that need repository secrets, or a write token those events cannot have,
must skip unless the head repository is this repository. Keep the fork-repository
guard for push and `workflow_dispatch` paths. Do not compare `github.repository`
to a hardcoded owner/name.

```yaml
if: >-
  github.event.repository.fork == false &&
  (
    github.event_name != 'pull_request' ||
    github.event.pull_request.head.repo.full_name == github.repository
  )
```

Drop the `repository.fork` clause when the job should still run on a fork's own
same-repository pull requests (for example cache cleanup). Drop the
`pull_request` clause when the job does not run on `pull_request`.

`workflow_run` report jobs are privileged in the base repository on purpose.
Do not add this guard there.

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

| Collect (`pull_request`) | Report (`workflow_run`) | Tools                       |
| ------------------------ | ----------------------- | --------------------------- |
| Cargo                    | Clippy report           | clippy                      |
| Actions lint             | Actions lint report     | actionlint, prettier        |
| Shell                    | Shell report            | shellcheck, shfmt           |
| Website                  | Website report          | prettier, eslint, stylelint |
| Docs lint                | Docs lint report        | rumdl                       |

New reviewdog jobs must follow the same collect/report pair. Keep
`pull-requests: write` on the report workflow only.

### Privileged checkout and trust

Report job order:

1. Check out the **base** repository to a dedicated path (e.g. `path: ci`) for
   trusted composites
2. Identify the PR (`number`, `base_ref`)
3. Check out the PR head to a **different** path (e.g. `path: pr`) via
   `checkout-workflow-run-pr-head` — pass `repository`, `ref`, `base_ref`, and
   `path` from the workflow
4. Download artifacts and pipe diagnostics into reviewdog — pass `workdir`
   matching the PR-head path, plus `commit`, `branch`, and `run_id`

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
<https://gh.io/securely-using-pull_request_target>.

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
