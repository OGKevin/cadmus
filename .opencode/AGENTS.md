# OpenCode plugin

The plugin in `plugins/cadmus.kobo/` registers the `cadmus.kobo` tools for
working with a Kobo over SSH: `cadmus.kobo.deploy` builds Cadmus for ARM,
assembles `dist/` and rsyncs it to the device, `cadmus.kobo.restart` sends
`SIGUSR1` so that `cadmus.sh` relaunches Cadmus with the deployed build. Both
take the device IP address and are in the tool catalog already, so there is
nothing to load by hand.

## Checks

The plugin is typechecked and tested like the rest of the code, from this
directory:

```sh
npm ci
npm run typecheck
npm test
```

`devenv` runs the same checks as `cadmus-opencode-check`.
