# OpenCode plugin

The plugin in `plugins/cadmus.kobo/` registers the `cadmus.kobo` tools for
working with a Kobo over SSH. `cadmus.kobo.deploy` always includes
`test,kobo` (pass extra Cargo features such as `tracing` on top), assembles `dist/`, and rsyncs it to `/mnt/onboard/.adds/cadmus-tst`.
`cadmus.kobo.restart` sends `SIGUSR1` to the Cadmus process whose cwd is that
install so `cadmus.sh` relaunches it. Both take the device IP address and are
in the tool catalog already, so there is nothing to load by hand.

Parse `execute` input with Zod (`.strict()`). Do not rely on the JSON schema
alone. Host names use `hostSchema` in `plugins/cadmus.kobo/common.ts`.

## Checks

The plugin is typechecked and tested like the rest of the code, from this
directory:

```sh
npm ci
npm run typecheck
npm test
npm run format
```

`devenv` runs the same checks as `cadmus-opencode-check`.
