# OpenCode plugin

The plugin in `plugins/cadmus.kobo/` registers the `cadmus.kobo` tools for
working with a Kobo over SSH. `cadmus.kobo.devices` browses mDNS and lists each
device's address with its platform, build kind, and version, so the address the
other tools need is discoverable. `cadmus.kobo.deploy` always includes
`test,kobo` (pass extra Cargo features such as `tracing` on top), assembles `dist/`, and rsyncs it to `/mnt/onboard/.adds/cadmus-tst`.
`cadmus.kobo.restart` sends `SIGUSR1` to the Cadmus process whose cwd is that
install so `cadmus.sh` relaunches it. Deploy and restart take the device IP
address; all three are in the tool catalog already, so there is nothing to load
by hand.

Parse `execute` input with Zod (`.strict()`). Do not rely on the JSON schema
alone. Host names use `hostSchema` in `plugins/cadmus.kobo/common.ts`.

## Checks

The plugin is typechecked and tested like the rest of the code, from this
directory:

```sh
npm ci
npm run typecheck
npm test
```

`devenv` runs the same checks as `cadmus-opencode-check`.
