# OpenCode plugin

The plugin in `plugins/cadmus.kobo/` registers the `cadmus.kobo` tools for
working with a Kobo over SSH. It is a thin argv wrapper: `deploy` and
`restart` shell out to `cadmuscli kobo`, and `devices` runs
`cadmuscli list-devices`. Keep SSH, rsync, build, and restart logic in
`xtask/src/tasks/kobo/`, not here.

- `cadmus.kobo.devices` runs `cadmuscli list-devices --json`, which browses
  mDNS and lists each device's address with its platform, build kind, and
  version, so the address the other tools need is discoverable.
- `cadmus.kobo.deploy` runs `cadmuscli kobo deploy [--ip <host>]`, which builds
  the pinned `test,kobo` feature set (plus optional extras such as `tracing`),
  assembles `dist/`, and syncs it with rsync to
  `/mnt/onboard/.adds/cadmus-tst`. Set `restart: true` to append `--restart`
  and relaunch Cadmus in the same call.
- `cadmus.kobo.restart` runs `cadmuscli kobo restart [--ip <host>]`, which sends
  `SIGUSR1` to the Cadmus process whose cwd is that install so `cadmus.sh`
  relaunches it.

The deploy and restart tools accept the SSH passthroughs `port`, `user`,
`identity`, `identitiesOnly`, `knownHosts`, `acceptNew`, and `sshConfig`; the
CLI resolves them on top of `~/.ssh/config`, and `IdentitiesOnly` in
`ssh_config` is not read unless `identitiesOnly` is passed. `ip` is optional:
when omitted, `cadmuscli` falls back to the `KOBO_IP` environment variable (set
it in `devenv.local.nix`). `KOBO_PASSWORD` is inherited from the environment and
is never passed as an argument. All three tools are already in the tool
catalog, so there is nothing to load by hand.

Parse `execute` input with Zod (`.strict()`). Do not rely on the JSON schema
alone. Host names use `hostSchema` in `plugins/cadmus.kobo/common.ts`.

## Checks

`devenv` runs the `package.json` typecheck, test, and format scripts as
`cadmus-opencode-check`.
