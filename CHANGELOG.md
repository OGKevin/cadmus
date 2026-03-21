# Changelog

## [0.10.0](https://github.com/OGKevin/cadmus/compare/v0.9.46...v0.10.0) (2026-03-21)

### ⚠ BREAKING CHANGES

- **Library:** With the introduction of SQLite for managing library data, there is no longer a need to set library mode to filesystem or (fake) database. It is all now stored into SQLite. This means this field is obsolete and has been removed.

### Features

- add global SQLite database ([#189](https://github.com/OGKevin/cadmus/issues/189)) ([6e98d66](https://github.com/OGKevin/cadmus/commit/6e98d66820f46ccaab3bbcc08dd995bdb5aa5649)), closes [#151](https://github.com/OGKevin/cadmus/issues/151)
- Embed documentation in binary ([#150](https://github.com/OGKevin/cadmus/issues/150)) ([d865103](https://github.com/OGKevin/cadmus/commit/d86510393c2ec73cdffa17f91c869db522f5546f)), closes [#112](https://github.com/OGKevin/cadmus/issues/112)
- **Kobo:** edit settings file during USB sharing ([#227](https://github.com/OGKevin/cadmus/issues/227)) ([c34a202](https://github.com/OGKevin/cadmus/commit/c34a202e0dc5ced467afa63dccb08d7b743c1a7d))
- **Library:** migrate library storage to SQLite ([#189](https://github.com/OGKevin/cadmus/issues/189)) ([6e98d66](https://github.com/OGKevin/cadmus/commit/6e98d66820f46ccaab3bbcc08dd995bdb5aa5649))
- **OTA:** adaptive chunk sizing based on observed throughput ([#228](https://github.com/OGKevin/cadmus/issues/228)) ([d0c9934](https://github.com/OGKevin/cadmus/commit/d0c9934ccc2d49eed12ddb374ce2cf58a5cf0c87))
- **OTA:** add default branch download support ([#131](https://github.com/OGKevin/cadmus/issues/131)) ([0c14f6c](https://github.com/OGKevin/cadmus/commit/0c14f6c953e2c14504d90f5632457d016fb0788b)), closes [#114](https://github.com/OGKevin/cadmus/issues/114)
- **OTA:** add GitHub device auth flow ([#170](https://github.com/OGKevin/cadmus/issues/170)) ([f934733](https://github.com/OGKevin/cadmus/commit/f934733ce4b727804b839281f66530d19dbdcb83)), closes [#169](https://github.com/OGKevin/cadmus/issues/169)
- **OTA:** support downloading stable releases ([#135](https://github.com/OGKevin/cadmus/issues/135)) ([377a087](https://github.com/OGKevin/cadmus/commit/377a087ac6453ecb4462e4cffd929721584a3283)), closes [#40](https://github.com/OGKevin/cadmus/issues/40)
- **OTA:** version check for stable releases [[#256](https://github.com/OGKevin/cadmus/issues/256)] ([85a4ae4](https://github.com/OGKevin/cadmus/commit/85a4ae45943add14b09be7c14152f950fd0fb1bf)), closes [#234](https://github.com/OGKevin/cadmus/issues/234)
- **Reader:** add go-to-next variant to FinishedAction ([#225](https://github.com/OGKevin/cadmus/issues/225)) ([2594a31](https://github.com/OGKevin/cadmus/commit/2594a3133e202bdf6348ededb6c57c0a7cffe1f2)), closes [#152](https://github.com/OGKevin/cadmus/issues/152)
- **Settings Editor:** add Telemetry category ([#251](https://github.com/OGKevin/cadmus/issues/251)) ([b9fb10c](https://github.com/OGKevin/cadmus/commit/b9fb10ca2bf995f8b905663a8e6e8d614af99663))
- **settings:** add versioning system ([#155](https://github.com/OGKevin/cadmus/issues/155)) ([70d402b](https://github.com/OGKevin/cadmus/commit/70d402bdf6713fbe5240eec68c3ef156292a3877)), closes [#56](https://github.com/OGKevin/cadmus/issues/56)
- **Telemetry:** test builds can log kernel logs ([#253](https://github.com/OGKevin/cadmus/issues/253)) ([c2d51a1](https://github.com/OGKevin/cadmus/commit/c2d51a17480c2558886a75c55db2eecac839694e))

### Bug Fixes

- **Kobo:** restart app on USB unplug after sharing ([#227](https://github.com/OGKevin/cadmus/issues/227)) ([c34a202](https://github.com/OGKevin/cadmus/commit/c34a202e0dc5ced467afa63dccb08d7b743c1a7d)), closes [#157](https://github.com/OGKevin/cadmus/issues/157)
- **Kobo:** set correct CWD in cadmus.sh restart loop ([#227](https://github.com/OGKevin/cadmus/issues/227)) ([c34a202](https://github.com/OGKevin/cadmus/commit/c34a202e0dc5ced467afa63dccb08d7b743c1a7d))
- **Library:** navigation bar when switching library ([#223](https://github.com/OGKevin/cadmus/issues/223)) ([b421f2b](https://github.com/OGKevin/cadmus/commit/b421f2b527d7e758b4fc0b7bd6e0df44a9181cce)), closes [#218](https://github.com/OGKevin/cadmus/issues/218)
- **OTA:** change UpdateMode from Gui to Full ([#174](https://github.com/OGKevin/cadmus/issues/174)) ([698c1ae](https://github.com/OGKevin/cadmus/commit/698c1ae9cfca51e10adf1a6442e7c0432fcb37c5))
- **OTA:** check if network is up before showing view ([#232](https://github.com/OGKevin/cadmus/issues/232)) ([1e6d7ef](https://github.com/OGKevin/cadmus/commit/1e6d7ef57a392b52d59cb0be0dde817eb2e00818)), closes [#68](https://github.com/OGKevin/cadmus/issues/68)
- **OTA:** close view when tapping outside of dialog ([#147](https://github.com/OGKevin/cadmus/issues/147)) ([ddfb738](https://github.com/OGKevin/cadmus/commit/ddfb7389d1447da1b658286d1d8729c5ec51747d))
- **OTA:** downloads on slow networks should be more reliable ([#228](https://github.com/OGKevin/cadmus/issues/228)) ([d0c9934](https://github.com/OGKevin/cadmus/commit/d0c9934ccc2d49eed12ddb374ce2cf58a5cf0c87))
- reported version in about dialog ([#160](https://github.com/OGKevin/cadmus/issues/160)) ([5973c84](https://github.com/OGKevin/cadmus/commit/5973c84833546e3879d9d8d3ea90d1baa4a11ed8))
- **settings editor:** library editor ([#205](https://github.com/OGKevin/cadmus/issues/205)) ([2739894](https://github.com/OGKevin/cadmus/commit/2739894c8651ca299291ae82a3a02c1141ea5d1a)), closes [#203](https://github.com/OGKevin/cadmus/issues/203)
- **USB:** redirect log writer to /tmp during USB share ([#265](https://github.com/OGKevin/cadmus/issues/265)) ([6ebf2f8](https://github.com/OGKevin/cadmus/commit/6ebf2f83ff786338f4515cbdce84449bdfb7c197)), closes [#246](https://github.com/OGKevin/cadmus/issues/246)

## [0.9.46](https://github.com/OGKevin/cadmus/compare/v0.9.45...v0.9.46) (2026-02-04)

### Features

- initial settings editor interface ([#41](https://github.com/OGKevin/cadmus/issues/41)) ([54267f0](https://github.com/OGKevin/cadmus/commit/54267f053253c0e8b708dcca3a22bc8ea55ecc06))
- PR test builds can be installed via OTA ([#57](https://github.com/OGKevin/cadmus/issues/57)) ([0dacb95](https://github.com/OGKevin/cadmus/commit/0dacb95512312277917c2f323760e79700b4c3a4))

## Cadmus Fork

This project is now maintained as **Cadmus**, a fork of the [Plato](https://github.com/baskerville/plato) document reader.

## [0.9.45](https://github.com/OGKevin/cadmus/compare/v0.9.44...v0.9.45) (2026-01-12)

### Features

- add restart application menu option ([#8](https://github.com/OGKevin/cadmus/issues/8)) ([4cf8af1](https://github.com/OGKevin/cadmus/commit/4cf8af12a03ecd7c74e86c575c7c84dfe51df358))

### Bug Fixes

- **fetcher:** add https support ([#39](https://github.com/OGKevin/cadmus/issues/39)) ([58b64f9](https://github.com/OGKevin/cadmus/commit/58b64f9a666cf52300a70a4331960b6e4e94abcc))
