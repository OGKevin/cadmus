# Installation

Cadmus ships as KoboRoot bundles. Each bundle targets a
different setup, so pick the variant that matches your device and tooling.

## Artifact variants

| Bundle                 | Includes                    | Cadmus path                     |
| ---------------------- | --------------------------- | ------------------------------- |
| `KoboRoot.tgz`         | Cadmus only (no NickelMenu) | `/mnt/onboard/.adds/cadmus`     |
| `KoboRoot-nm.tgz`      | Cadmus + NickelMenu         | `/mnt/onboard/.adds/cadmus`     |
| `KoboRoot-test.tgz`    | Test build only             | `/mnt/onboard/.adds/cadmus-tst` |
| `KoboRoot-nm-test.tgz` | Test build + NickelMenu     | `/mnt/onboard/.adds/cadmus-tst` |

## Pick the right bundle

- Use non-test bundles for normal installs.
- Use test bundles when following a PR or CI test build.
- Choose NickelMenu variants if you rely on NickelMenu integration.
