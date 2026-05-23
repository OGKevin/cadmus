# Changelog

## [0.10.1](https://github.com/OGKevin/cadmus/compare/v0.10.0...v0.10.1) (2026-05-23)

### ⚠ BREAKING CHANGES

- **dictionary:** Fuzzy dictionary search no longer corrects first-character typos Fuzzy word lookup now uses a 3-character prefix pre-filter for performance. Searches where the query and the target word differ in the first three characters will no longer return results. For example, searching `"bas"` will not suggest `"bar"`.
- **library:** hashes are now calculated using BLAKE3 instead of the custom implementation using mtime which caused inconsistent hashing. BLAKE3 is more CPU and battery intensive, and slower.

### Features

- add suspend and power off to exit menu ([#330](https://github.com/OGKevin/cadmus/issues/330)) ([6cb9052](https://github.com/OGKevin/cadmus/commit/6cb905244e8edebdd227c17d8743c5c6bc5b8cf0))
- add WiFi status monitor for Kobo devices ([#324](https://github.com/OGKevin/cadmus/issues/324)) ([d89ccaa](https://github.com/OGKevin/cadmus/commit/d89ccaaa5302b1c1f80454f00abccccdc3f82bca))
- **cadmus:** exit to nickel after 3 consecutive crashes ([#295](https://github.com/OGKevin/cadmus/issues/295)) ([253edbe](https://github.com/OGKevin/cadmus/commit/253edbe8958a44d108676d57b85942f21bb7c899)), closes [#272](https://github.com/OGKevin/cadmus/issues/272)
- **dictionaries:** Add download state tracking ([#396](https://github.com/OGKevin/cadmus/issues/396)) ([da509ae](https://github.com/OGKevin/cadmus/commit/da509aef81d0a0a55b39d336919a633dc4c5a419))
- **dictionaries:** add native monolingual dictionary support ([#378](https://github.com/OGKevin/cadmus/issues/378)) ([9a901a5](https://github.com/OGKevin/cadmus/commit/9a901a5e22dbc78ff4e88186cafa4a4957c8c5f5))
- **dictionary:** index files into SQLite ([#447](https://github.com/OGKevin/cadmus/issues/447)) ([ef75769](https://github.com/OGKevin/cadmus/commit/ef75769e8285f231c4af188cc6aa195b22c72c3a))
- **dictionary:** track installed version with cache invalidation ([#395](https://github.com/OGKevin/cadmus/issues/395)) ([8de943d](https://github.com/OGKevin/cadmus/commit/8de943d7bbff001445333bc624e086e7b5653235))
- **i18n:** add i18n support for UI strings ([#289](https://github.com/OGKevin/cadmus/issues/289)) ([235c494](https://github.com/OGKevin/cadmus/commit/235c4943e17398988b4652298f0b61771cad885e))
- **Intermission:** add blank screens ([#483](https://github.com/OGKevin/cadmus/issues/483)) ([75add0d](https://github.com/OGKevin/cadmus/commit/75add0d9822eef0510afdc0905a5b72f33c56fe9))
- **intermission:** add calendar intermission screen ([#402](https://github.com/OGKevin/cadmus/issues/402)) ([3f36f25](https://github.com/OGKevin/cadmus/commit/3f36f258673267305fc5326154f4140f4742a448))
- **library:** Library import is no async. ([7fbf304](https://github.com/OGKevin/cadmus/commit/7fbf304fdfb84df1d4c6fcd661adc80ef12c66bc))
- **library:** switch fingerprints to BLAKE3 content hash ([#385](https://github.com/OGKevin/cadmus/issues/385)) ([7b03de3](https://github.com/OGKevin/cadmus/commit/7b03de3cac4f79451e7d56b8e0a63d325556454d)), closes [#184](https://github.com/OGKevin/cadmus/issues/184)
- **Settigns Editor:** add refresh rate settings ([#478](https://github.com/OGKevin/cadmus/issues/478)) ([58cb13e](https://github.com/OGKevin/cadmus/commit/58cb13e17cc50d866ad3fa276f6c549b92049dd9))
- **settings editor:** add pagination to CategoryEditor ([#377](https://github.com/OGKevin/cadmus/issues/377)) ([037c24c](https://github.com/OGKevin/cadmus/commit/037c24cef9a433b59d523665ced3384e7a564948))
- **Settings Editor:** all settings fields are now translatable ([51fa0e9](https://github.com/OGKevin/cadmus/commit/51fa0e9f130ab03dccd61a9c57a3b3e5c2f0b437))
- **settings editor:** expose import settings ([#341](https://github.com/OGKevin/cadmus/issues/341)) ([5dc926e](https://github.com/OGKevin/cadmus/commit/5dc926e34c142a88feafd5b2cefb3a1bff58b581))

### Bug Fixes

- **kobo:** wake the touch layer on resume ([025a013](https://github.com/OGKevin/cadmus/commit/025a0137921fd935073b8d2f0c9b255078782aa9))
- **Library:** remove books with empty paths on import ([#485](https://github.com/OGKevin/cadmus/issues/485)) ([eb6f2a8](https://github.com/OGKevin/cadmus/commit/eb6f2a880206829888d73d0aadc536f8b8d20d67))
- **library:** use natural sort order ([#370](https://github.com/OGKevin/cadmus/issues/370)) ([f053a28](https://github.com/OGKevin/cadmus/commit/f053a287dbbd74c3195241401347d6d09401b319)), closes [#297](https://github.com/OGKevin/cadmus/issues/297)
- **OTA:** use Cadmus tmp dir for OTA downloads ([#460](https://github.com/OGKevin/cadmus/issues/460)) ([6fab681](https://github.com/OGKevin/cadmus/commit/6fab6819ceda574c4c95910fabf0887d5612254d))
- **settings editor:** add hold gesture for library delete ([#365](https://github.com/OGKevin/cadmus/issues/365)) ([dbd5f1b](https://github.com/OGKevin/cadmus/commit/dbd5f1beaa22c80648a8f6c2068727b2bf908091)), closes [#353](https://github.com/OGKevin/cadmus/issues/353)
- **settings editor:** wrap category nav bar buttons onto 2 rows ([#379](https://github.com/OGKevin/cadmus/issues/379)) ([0848a71](https://github.com/OGKevin/cadmus/commit/0848a719235140ced28d2d5f13c220cff470f9b2))
- **Top Menu:** make restart and reboot clearer ([#293](https://github.com/OGKevin/cadmus/issues/293)) ([402e42d](https://github.com/OGKevin/cadmus/commit/402e42d7b7f63e5403a3f197d8c334d5f92863a2)), closes [#292](https://github.com/OGKevin/cadmus/issues/292)
- **WiFi:** going from Nickel to Cadmus does not interrupt WiFi connection ([6bfd7b4](https://github.com/OGKevin/cadmus/commit/6bfd7b4783fa69486b56d07d1568675fcb7a106e))
- **WiFi:** previous DHCP leases will now be re-used, resulting in stable IP addresses. ([6bfd7b4](https://github.com/OGKevin/cadmus/commit/6bfd7b4783fa69486b56d07d1568675fcb7a106e))

### Performance Improvements

- Library sorting is now precomputed instead of calculated on demand. Should benefit big libraries. ([93cb8a1](https://github.com/OGKevin/cadmus/commit/93cb8a1c5fcdd38566649798e5dfc4a5d8a79d55))
- Memory usage should reduce a tiny bit, as the whole library is no longer loaded in memory. Memory pressure reduction depends on how big the library is to begin with. This will benefit folks with huge libraries. ([93cb8a1](https://github.com/OGKevin/cadmus/commit/93cb8a1c5fcdd38566649798e5dfc4a5d8a79d55))
- optimize dictionary loading ([#364](https://github.com/OGKevin/cadmus/issues/364)) ([5b23c62](https://github.com/OGKevin/cadmus/commit/5b23c62629a96e7c250e476713f651a41567b06b))
- **startup:** Library import is now async, this means that it no longer blocks startup. ([7fbf304](https://github.com/OGKevin/cadmus/commit/7fbf304fdfb84df1d4c6fcd661adc80ef12c66bc))
- **Startup:** Wifi management on startup is now async, instead of sync. This should improve startup speeds. ([6bfd7b4](https://github.com/OGKevin/cadmus/commit/6bfd7b4783fa69486b56d07d1568675fcb7a106e))

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
