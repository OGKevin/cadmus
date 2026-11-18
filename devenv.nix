{
  pkgs,
  config,
  inputs,
  ...
}:

let
  inherit (pkgs.stdenv) isLinux;
  inherit (pkgs.stdenv) isDarwin;

  # `otelcol-contrib` 0.155.0 pins a `cockroachdb/swiss` revision that stopped
  # compiling with Go 1.27, nixpkgs' default. nixpkgs pins the older builder for
  # packages that have not caught up yet (see `teleport_18`); do the same until
  # the collector release bumps that dependency.
  otelcol = (pkgs.opentelemetry-collector-releases.override {
    buildGoModule = pkgs.buildGo126Module;
    go = pkgs.go_1_26;
  }).otelcol-contrib;

  # Single source for the Rust toolchain pin; Renovate's custom.regex manager
  # (renovate.jsonc) bumps it together with the CI `toolchain:` pins.
  rustVersion = "1.100.0";

  openHtml =
    path:
    if isLinux then
      "xdg-open ${path}"
    else if isDarwin then
      "open ${path}"
    else
      "echo \"Open ${path} in a browser\"";

  # Shared with CI (.github/workflows/actions-lint.yml) so the ignore list
  # only needs to be updated in one place.
  actionlintIgnorePatterns = builtins.filter (line: line != "" && !pkgs.lib.hasPrefix "#" line) (
    pkgs.lib.splitString "\n" (builtins.readFile ./.github/lint/actionlint-ignore.txt)
  );

  # Rust 1.94+ ships `lib/` in the component tarballs as a symlink rather than
  # a directory. `symlinkJoin` merges with `lndir`, which cannot write into a
  # symlinked destination: it warns "lib: is a link instead of a directory" and
  # leaves the link in place, so the remaining components' `lib` contents are
  # never merged. mk-aggregated's `postBuild` then runs
  # `cp --remove-destination` on paths under `$out/lib`, which resolves to the
  # same store path as the source and fails with "are the same file".
  #
  # Resolve `$out/lib` into a real directory at the top of each merge
  # iteration, so every later component is merged normally. It has to happen
  # inside the loop rather than in `postBuild`, which nixpkgs appends after the
  # merge: by then `lndir` has already skipped the other components.
  #
  # The anchor below is nixpkgs' merge loop and must track it. `replaceStrings`
  # returns its input unchanged when the pattern is absent, so a stale anchor
  # makes this a silent no-op rather than an error — that is how the fix came
  # to be "present but inert" on macOS. The assert turns that silent no-op into
  # a hard evaluation error.
  fixRustToolchainLibSymlink =
    drv:
    drv.overrideAttrs (old: {
      buildCommand =
        let
          mergeLoop = "for i in \"\${paths[@]}\"; do\n";
          patched =
            builtins.replaceStrings
              [ mergeLoop ]
              [
                (
                  mergeLoop
                  + "  if [ -L \"$out/lib\" ]; then\n"
                  + "    libTarget=$(${pkgs.coreutils}/bin/readlink -f \"$out/lib\")\n"
                  + "    rm \"$out/lib\"\n"
                  + "    mkdir \"$out/lib\"\n"
                  + "    ${pkgs.lndir}/bin/lndir -silent \"$libTarget\" \"$out/lib\"\n"
                  + "  fi\n"
                )
              ]
              old.buildCommand;
        in
        assert patched != old.buildCommand
          || throw "fixRustToolchainLibSymlink: nixpkgs symlinkJoin no longer contains the merge loop this patch anchors on; update mergeLoop";
        patched;
    });

  # Build the stable Rust toolchain the way devenv does, with the lib-symlink
  # fix applied. Replicates devenv's `channel != "nixpkgs"` wiring so the fix
  # can wrap the derivation before devenv sees it.
  darwinRustToolchainFix = pkgs.lib.optionalAttrs isDarwin (
    let
        rustOverlaySrc = inputs.rust-overlay;
        rustBin = rustOverlaySrc.lib.mkRustBin { } pkgs.buildPackages;

        mkAggregatedFn = import (rustOverlaySrc + "/lib/mk-aggregated.nix");
        mkAggregatedArgs = builtins.functionArgs mkAggregatedFn;
        mkAggregated = mkAggregatedFn (
          {
            inherit (pkgs)
              lib
              stdenv
              symlinkJoin
              bash
              curl
              ;
            inherit (pkgs.buildPackages) rustc;
            pkgsTargetTarget = pkgs.targetPackages;
          }
          // pkgs.lib.optionalAttrs (mkAggregatedArgs ? makeWrapper) { inherit (pkgs) makeWrapper; }
          // pkgs.lib.optionalAttrs (mkAggregatedArgs ? pkgsHostHost) { inherit (pkgs) pkgsHostHost; }
        );

        toolchain = rustBin.stable.${rustVersion};
        nativeTarget = pkgs.stdenv.hostPlatform.rust.rustcTargetSpec;
        allTargets = pkgs.lib.unique ([ nativeTarget ] ++ [ "arm-unknown-linux-gnueabihf" ]);
        components = [
          "cargo"
          "clippy"
          "llvm-tools-preview"
          "rust-src"
          "rustc"
          "rustfmt"
        ];

        availableComponents = toolchain._manifest.profiles.complete or [ ];
        allComponents = toolchain._components or { };

        targetComponents = builtins.map (
          target:
          let
            targetComponentSet = allComponents.${target} or { };
            targetRustStd = targetComponentSet.rust-std or null;
          in
          targetRustStd
        ) allTargets;

        resolvedComponents = builtins.map (
          c:
          let
            resolvedName =
              if builtins.elem c availableComponents then
                c
              else if builtins.elem "${c}-preview" availableComponents then
                "${c}-preview"
              else
                throw "Component '${c}' not found";
            toolchainComponents = builtins.removeAttrs toolchain [ "rust" ];
          in
          toolchainComponents.${resolvedName}
        ) components;

        allSelectedComponents = resolvedComponents ++ targetComponents;

        profile = mkAggregated {
          pname = "rust-stable-${toolchain._manifest.version}";
          inherit (toolchain._manifest) version date;
          selectedComponents = allSelectedComponents;
        };
    in
    {
      rustToolchain = fixRustToolchainLibSymlink profile;
      rustSrcPath = "${toolchain.rust-src}/lib/rustlib/src/rust/library";
    }
  );

  # cargo-diff-tools with Rust 1.70 for clap v2 compatibility.
  # Pinned nixpkgs still defaults fetchCrate to the crates.io API, which
  # returns 403 to Nix's curl User-Agent. Fetch from the static CDN instead
  # (NixOS/nixpkgs#525067).
  cargo-diff-tools =
    let
      rustPlatform170 = pkgs.makeRustPlatform {
        rustc = pkgs.rust-bin.stable."1.70.0".default;
        cargo = pkgs.rust-bin.stable."1.70.0".default;
      };
    in
    rustPlatform170.buildRustPackage rec {
      pname = "cargo-diff-tools";
      version = "0.1.2";

      src = pkgs.fetchCrate {
        inherit pname version;
        sha256 = "1a6878v73zx9kx31jcyzf9gks8dfb1074xk4qhy3xr2gfx2pkmv4";
        registryDl = "https://static.crates.io/crates";
      };

      cargoHash = "sha256-sy1b/bIIsG5eyR0medE5Ztv39jI2HtWeiVc207ViYCA=";
      doCheck = false;
    };

  # Linaro GCC toolchain for Kobo - same as used by Kobo Reader
  # https://github.com/kobolabs/Kobo-Reader/blob/master/toolchain/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz
  # On macOS, we download the Darwin-compatible Linaro compiler from Google Drive.
  linaroToolchain = pkgs.stdenv.mkDerivation {
    pname = "gcc-linaro";
    version = "4.9.4-2017.01";

    src =
      if isLinux then
        pkgs.fetchurl {
          url = "https://developer.arm.com/-/cdn-downloads/permalink/legacy-linaro-gnu-toolchains/4.9-2017.01/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz";
          sha256 = "22914118fd963f953824b58107015c6953b5bbdccbdcf25ad9fd9a2f9f11ac07";
        }
      else
        pkgs.fetchurl {
          name = "gcc-linaro-darwin.tar.bz2";
          url = "https://drive.usercontent.google.com/download?id=1ggMLM3VBwCYQuFTpJEC0OmyMkiDtYMju&export=download&confirm=t";
          sha256 = "rSP4JS/KsK8dxPwvdY7Cnb5zxbKbFYnVuKe/VIOIf/Q=";
        };

    nativeBuildInputs = pkgs.lib.optionals isLinux [ pkgs.autoPatchelfHook ];
    buildInputs = pkgs.lib.optionals isLinux [
      pkgs.stdenv.cc.cc.lib
      pkgs.zlib
      pkgs.ncurses5
      pkgs.expat
      pkgs.xz
    ];

    dontConfigure = true;
    dontBuild = true;

    installPhase = ''
      mkdir -p $out
      cp -r * $out/
    '';

    # Pre-built binaries only need patching/stripping on Linux
    autoPatchelfIgnoreMissingDeps = pkgs.lib.optionals isLinux [ "libpython2.7.so.1.0" ];
    dontFixup = isDarwin;
    dontPatchELF = isDarwin;
    dontStrip = isDarwin;
  };

  # Custom mdbook-epub from specific commit
  mdbook-epub-custom = pkgs.rustPlatform.buildRustPackage rec {
    pname = "mdbook-epub";
    version = "21a1c813";

    src = pkgs.fetchFromGitHub {
      owner = "Michael-F-Bryan";
      repo = "mdbook-epub";
      rev = "21a1c8134134201a2d555313447c96e56e2a8996";
      hash = "sha256-7QxIggAioJ92iCPCxs2ZwtML3OtCcg0h2/kvTNMB/pw=";
    };

    cargoHash = "sha256-yxB1PMbc7Ck+PEAm/v/BrC6xMTi6jb1uLH++/whiKFU=";

    nativeBuildInputs = [ pkgs.pkg-config ];

    buildInputs = [ pkgs.bzip2 ];

    # Tests are broken upstream
    doCheck = false;
  };

  mdbook-i18n-helpers-custom = pkgs.rustPlatform.buildRustPackage {
    pname = "mdbook-i18n-helpers";
    version = "0.4.0-ogkevin";

    # Nix cannot use the git submodule checkout here. Pin the same commit as
    # thirdparty/mdbook-i18n-helpers via fetchFromGitHub; Renovate keeps rev in
    # sync with the submodule — update hash (and cargoHash if needed).
    src = pkgs.fetchFromGitHub {
      owner = "ogkevin";
      repo = "mdbook-i18n-helpers";
      rev = "0ddd35244156456c0a1a785306b8ecf469a067f6";
      hash = "sha256-AE2JTIO8fgGshma6PDQTUBh12m2qaZdYoO7WiAw9iC8=";
    };

    # Use the below code with `devenv --impure` for local development
    # src = builtins.path {
    #   path = "${config.devenv.root}/thirdparty/mdbook-i18n-helpers";
    #   name = "mdbook-i18n-helpers-src";
    # };

    cargoHash = "sha256-LG5au6TQK2XnLswr/fBLwE6hLfz9/YJQ5MOunvVRHZw=";
    cargoBuildFlags = [
      "-p"
      "mdbook-i18n-helpers"
    ];

    doCheck = true;
  };

  # Base ports only; devenv allocates upward from these when the manager runs
  # (and falls back to the base port otherwise). Every config, probe, env var
  # and banner below reads the resolved `.value`, so a port taken by another
  # worktree moves the service and its consumers together.
  tempoPorts = config.processes.tempo.ports;
  lokiPorts = config.processes.loki.ports;
  pyroscopePorts = config.processes.pyroscope.ports;
  grafanaPorts = config.processes.grafana.ports;
  prometheusPorts = config.processes.prometheus.ports;
  otelPorts = config.processes.opentelemetry-collector.ports;
  websitePorts = config.processes.website.ports;
  mdbookPorts = config.processes.mdbook.ports;
  storybookPorts = config.processes.storybook.ports;

  # Grafana datasource provisioning
  grafanaDatasources = pkgs.writeText "datasources.yaml" ''
    apiVersion: 1
    datasources:
      - name: Tempo
        type: tempo
        access: proxy
        url: http://localhost:${toString tempoPorts.http.value}
        isDefault: false
        editable: true

      - name: Loki
        type: loki
        access: proxy
        url: http://localhost:${toString lokiPorts.http.value}
        isDefault: false
        editable: true

      - name: Prometheus
        type: prometheus
        access: proxy
        url: http://localhost:${toString prometheusPorts.main.value}
        isDefault: true
        editable: true

      - name: Pyroscope
        type: grafana-pyroscope-datasource
        access: proxy
        url: http://localhost:${toString pyroscopePorts.http.value}
        isDefault: false
        editable: true
  '';
in
{
  # Overlays for platform-specific fixes
  overlays = [
    # macOS: Fix GDB 17.1 build failure with Clang (nixpkgs https://github.com/NixOS/nixpkgs/issues/483562)
    (
      final: prev:
      prev.lib.optionalAttrs prev.stdenv.hostPlatform.isDarwin {
        gdb = prev.gdb.overrideAttrs (old: {
          configureFlags = builtins.filter (f: f != "--enable-werror") (old.configureFlags or [ ]);
        });
      }
    )
    inputs.rust-overlay.overlays.default
  ];

  packages = [
    # Basic tools required by build scripts
    pkgs.git
    pkgs.wget
    pkgs.curl
    pkgs.pkg-config
    pkgs.unzip
    pkgs.jq
    pkgs.yamllint
    pkgs.check-jsonschema

    pkgs.mdbook
    mdbook-epub-custom
    pkgs.mdbook-mermaid
    mdbook-i18n-helpers-custom
    pkgs.gettext

    cargo-diff-tools
    pkgs.cargo-nextest
    pkgs.reviewdog
    pkgs.cargo-expand
    pkgs.sccache
    pkgs.gnuplot

    # C/C++ build tools for compiling thirdparty libraries
    pkgs.gnumake
    pkgs.cmake
    pkgs.meson
    pkgs.ninja
    pkgs.autoconf
    pkgs.automake
    pkgs.libtool
    pkgs.gperf
    pkgs.python3
    pkgs.tcl

    # Libraries for native builds (emulator/tests)
    pkgs.djvulibre
    pkgs.freetype
    pkgs.harfbuzz

    # Emulator dependency
    pkgs.sdl3

    # Native build dependencies (development headers)
    pkgs.zlib
    pkgs.bzip2
    pkgs.libpng
    pkgs.libjpeg
    pkgs.openjpeg
    pkgs.jbig2dec
    pkgs.gumbo

    # SQLx CLI for database migrations and compile-time query verification
    pkgs.sqlx-cli
    pkgs.sqlite
    pkgs.sqlitebrowser

    pkgs.wrangler

    pkgs.cargo-llvm-cov
    pkgs.python3Packages.diff-cover
    pkgs.xdg-utils

    # Linaro ARM cross-compilation toolchain (provides arm-linux-gnueabihf-* commands)
    linaroToolchain

    # patchelf is used to patch ELF binaries
    pkgs.patchelf
  ]
  # Linux-only packages
  ++ pkgs.lib.optionals isLinux [
    # GCC - on macOS we use clang from Xcode
    pkgs.gcc

    # SDL3 runtime backends (Wayland / X11 / GL)
    pkgs.libGL
    pkgs.libglvnd
    pkgs.mesa
    pkgs.wayland
    pkgs.libxkbcommon
    pkgs.libdecor
    pkgs.xorg.libX11
    pkgs.xorg.libXext
    pkgs.xorg.libXcursor
    pkgs.xorg.libXi
    pkgs.xorg.libXrandr
    pkgs.xorg.libXfixes
    pkgs.xorg.libXrender
    pkgs.vulkan-loader
  ]
  # macOS-specific packages
  ++ pkgs.lib.optionals isDarwin [
    # macOS uses Apple's clang from Xcode Command Line Tools
    # Frameworks are provided by the system SDK, no need to add them explicitly
  ];

  # Enable Rust with cross-compilation support
  languages = {
    javascript = {
      enable = true;
      npm = {
        enable = true;
        install.enable = true;
      };
    };
    rust = {
      enable = true;
      channel = "stable";
      version = rustVersion;
      targets = [ "arm-unknown-linux-gnueabihf" ];
      components = [
        "cargo"
        "clippy"
        "llvm-tools-preview"
        "rustc"
        "rustfmt"
      ];
      rustdocflags = "-D warnings";
      # The lib-symlink merge failure is macOS-only; on Linux leaving this
      # alone lets devenv build the toolchain itself. Enforced by the
      # macos-latest job in .github/workflows/devenv-test.yml.
      toolchainPackage = pkgs.lib.mkIf isDarwin (pkgs.lib.mkForce darwinRustToolchainFix.rustToolchain);
    };
  };

  dotenv.enable = true;

  env = {
    TEST_ROOT_DIR = config.devenv.root;
    CADMUS_WORKSPACE_ROOT = config.devenv.root;
    # Match `RUST_MIN_STACK_BYTES` in xtask `test.rs` (3 * 1024 * 1024 bytes).
    RUST_MIN_STACK = toString (3 * 1024 * 1024);

    RUST_LOG = "cadmus_core=trace,cadmus=trace";
    RUST_BACKTRACE = "1";
    # Resolved from `processes.*.ports` so the shell and the running stack
    # agree even when a base port is taken. Docs point at these, not literals.
    OTEL_EXPORTER_OTLP_ENDPOINT = "http://127.0.0.1:${toString otelPorts.http.value}";
    PYROSCOPE_SERVER_URL = "http://127.0.0.1:${toString pyroscopePorts.http.value}";
    LOKI_URL = "http://127.0.0.1:${toString lokiPorts.http.value}";
    TEMPO_URL = "http://127.0.0.1:${toString tempoPorts.http.value}";
    PROMETHEUS_URL = "http://127.0.0.1:${toString prometheusPorts.main.value}";
    GRAFANA_URL = "http://127.0.0.1:${toString grafanaPorts.http.value}";
    WEBSITE_URL = "http://127.0.0.1:${toString websitePorts.http.value}";
    MDBOOK_URL = "http://127.0.0.1:${toString mdbookPorts.http.value}";
    STORYBOOK_URL = "http://127.0.0.1:${toString storybookPorts.http.value}";
    NEXTEST_NO_TESTS = "pass";

    # Default branch for patch coverage (cadmus-coverage-diff). Override in devenv.local.nix.
    CADMUS_DEFAULT_BRANCH = "master";
    CARGO_TARGET_DIR = "${config.devenv.root}/target";

    # jemalloc configure uses -O0 in debug builds, which triggers _FORTIFY_SOURCE warnings
    # treated as errors under Nix hardening. Disable hardening for jemalloc's build script.
    NIX_HARDENING_ENABLE = "";

    # pkg-config configuration for cross-compilation
    PKG_CONFIG_ALLOW_CROSS = "1";

    # Cargo linker for ARM target (only used when building for ARM)
    CARGO_TARGET_ARM_UNKNOWN_LINUX_GNUEABIHF_LINKER = "arm-linux-gnueabihf-gcc";

    # C compiler for ARM target (used by cc crate for build scripts)
    CC_arm_unknown_linux_gnueabihf = "arm-linux-gnueabihf-gcc";
    AR_arm_unknown_linux_gnueabihf = "arm-linux-gnueabihf-ar";

    # Point libsqlite3-sys at the custom SQLite build (cadmuscli setup).
    # Absolute paths are required: build scripts run in the crate registry
    # directory, not the workspace root, so relative paths don't resolve.
    #
    # SQLITE3_LIB_DIR is the non-target-aware fallback used by native builds.
    # For cross-compilation, PKG_CONFIG_PATH_<target> gives each build
    # (host proc-macro + ARM target) its own sqlite via pkg-config.
    SQLITE3_STATIC = "1";
    "PKG_CONFIG_PATH_${builtins.replaceStrings ["-"] ["_"] pkgs.stdenv.hostPlatform.rust.rustcTargetSpec}" =
      "${config.devenv.root}/target/cadmus-build-deps/${pkgs.stdenv.hostPlatform.rust.rustcTargetSpec}/sqlite/lib/pkgconfig";
    PKG_CONFIG_PATH_arm_unknown_linux_gnueabihf =
      "${config.devenv.root}/target/cadmus-build-deps/arm-unknown-linux-gnueabihf/sqlite/lib/pkgconfig";
    PKG_CONFIG_arm_unknown_linux_gnueabihf = "${pkgs.pkg-config-unwrapped}/bin/pkg-config";

    # bindgen (used by libsqlite3-sys) requires LIBCLANG_PATH to locate
    # libclang. In the Nix devenv the clang wrapper on PATH does not have a
    # sibling lib/ directory, so we point directly at the clang-tools package
    # that ships libclang.dylib / libclang.so.
    LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

    RUSTC_WRAPPER = "${pkgs.sccache}/bin/sccache";
    SCCACHE_CACHE_SIZE = "50G";

    # sccache does not cache incremental compiles, and cargo enables incremental
    # compilation for the dev profile by default.
    CARGO_INCREMENTAL = "0";

    # Keep the daemon private to this devenv: clients without this variable look
    # for a TCP server on 127.0.0.1:4226 instead of this socket, so nothing
    # outside the shell can reach it and other projects cannot collide with it.
    SCCACHE_SERVER_UDS = "${config.devenv.state}/sccache.sock";

    # sccache does not create parent directories, and an unopenable log path
    # makes server startup fail, which fails the build.
    SCCACHE_ERROR_LOG = "${config.devenv.state}/sccache-server.log";
  }
  // pkgs.lib.optionalAttrs isDarwin {
    # devenv only derives this when it builds the toolchain itself, so an
    # overridden `toolchainPackage` (see `darwinRustToolchainFix`) leaves it
    # null; use the `rust-src` component merged into our toolchain.
    RUST_SRC_PATH = pkgs.lib.mkForce darwinRustToolchainFix.rustSrcPath;
  };

  services.opentelemetry-collector = {
    enable = true;
    package = otelcol;
    settings = {
      # Literal base ports: the module reads these to allocate the listener
      # ports, so they must not reference the resolved values.
      receivers.otlp.protocols = {
        grpc.endpoint = "0.0.0.0:4317";
        http.endpoint = "0.0.0.0:4318";
      };

      processors.batch = { };

      exporters = {
        "otlp/tempo" = {
          endpoint = "localhost:${toString tempoPorts.otlpGrpc.value}";
          tls.insecure = true;
        };

        "otlphttp/loki" = {
          endpoint = "http://localhost:${toString lokiPorts.http.value}/otlp";
        };

        prometheus = {
          endpoint = "0.0.0.0:${toString otelPorts.exporter.value}";
        };

        debug = {
          verbosity = "basic";
        };
      };

      service.pipelines = {
        traces = {
          receivers = [ "otlp" ];
          processors = [ "batch" ];
          exporters = [
            "otlp/tempo"
            "debug"
          ];
        };

        logs = {
          receivers = [ "otlp" ];
          processors = [ "batch" ];
          exporters = [ "otlphttp/loki" ];
        };

        metrics = {
          receivers = [ "otlp" ];
          processors = [ "batch" ];
          exporters = [ "prometheus" ];
        };
      };
    };
  };

  services.prometheus = {
    enable = true;
    port = 9090;

    storage = {
      path = "${config.devenv.state}/prometheus";
      retentionTime = "1h";
    };

    globalConfig = {
      scrape_interval = "15s";
      evaluation_interval = "15s";
    };

    scrapeConfigs = [
      {
        job_name = "otel-collector";
        static_configs = [
          {
            targets = [ "localhost:${toString otelPorts.exporter.value}" ];
          }
        ];
      }
      {
        job_name = "prometheus";
        static_configs = [
          {
            targets = [ "localhost:${toString prometheusPorts.main.value}" ];
          }
        ];
      }
    ];
  };

  # Dev-stack processes; every listener port is allocated so concurrent
  # worktrees move instead of failing.
  processes = {
    # The module allocates the health, OTLP and self-metrics listener ports
    # from the base ports in `settings`, and derives its own readiness probe.
    opentelemetry-collector = {
      ports.exporter.allocate = 8889;

      after = [
        "devenv:processes:tempo"
        "devenv:processes:loki"
      ];
    };

    tempo = {
      ports = {
        http.allocate = 3200;
        grpc.allocate = 9096;
        otlpGrpc.allocate = 4327;
        otlpHttp.allocate = 4328;
      };

      ready = {
        initial_delay = 3;
        period = 5;
        probe_timeout = 3;
        http.get = {
          port = tempoPorts.http.value;
          path = "/ready";
        };
      };

      shutdown.grace = 20;

      exec = ''
        mkdir -p ${config.devenv.state}/tempo/{traces,wal,live-store,shutdown-marker}

        exec ${pkgs.tempo}/bin/tempo \
          -config.file=${pkgs.writeText "tempo.yaml" ''
            server:
              http_listen_port: ${toString tempoPorts.http.value}
              grpc_listen_port: ${toString tempoPorts.grpc.value}
              grpc_server_max_recv_msg_size: 104857600
              grpc_server_max_send_msg_size: 104857600

            distributor:
              receivers:
                otlp:
                  protocols:
                    grpc:
                      endpoint: 0.0.0.0:${toString tempoPorts.otlpGrpc.value}
                    http:
                      endpoint: 0.0.0.0:${toString tempoPorts.otlpHttp.value}

            live_store:
              wal:
                path: ${config.devenv.state}/tempo/live-store
              shutdown_marker_dir: ${config.devenv.state}/tempo/shutdown-marker
              max_trace_idle: 10s
              max_block_duration: 5m

            memberlist:
              bind_addr:
                - 127.0.0.1
              abort_if_cluster_join_fails: false

            backend_scheduler:
              local_work_path: ${config.devenv.state}/tempo/work

            backend_worker:
              compaction:
                block_retention: 1h

            storage:
              trace:
                backend: local
                local:
                  path: ${config.devenv.state}/tempo/traces
                wal:
                  path: ${config.devenv.state}/tempo/wal

            overrides:
              defaults:
                global:
                  max_bytes_per_trace: 104857600

            querier:
              frontend_worker:
                grpc_client_config:
                  max_send_msg_size: 104857600

            query_frontend:
              search:
                max_duration: 1h
          ''} \
          -target=all
      '';
    };

    loki = {
      ports = {
        http.allocate = 3100;
        grpc.allocate = 9097;
      };

      # The first probe after an idle period gets 503 ("Ingester not ready:
      # waiting for 15s after being ready"), so allow time and retries.
      ready = {
        initial_delay = 10;
        period = 5;
        probe_timeout = 3;
        failure_threshold = 6;
        http.get = {
          port = lokiPorts.http.value;
          path = "/ready";
        };
      };

      shutdown.grace = 20;

      exec = ''
        mkdir -p ${config.devenv.state}/loki/{index,cache,chunks,wal,compactor}

        exec ${pkgs.grafana-loki}/bin/loki \
          -config.file=${pkgs.writeText "loki.yaml" ''
            auth_enabled: false

            server:
              http_listen_port: ${toString lokiPorts.http.value}
              grpc_listen_port: ${toString lokiPorts.grpc.value}

            common:
              path_prefix: ${config.devenv.state}/loki
              replication_factor: 1
              ring:
                kvstore:
                  store: inmemory

            ingester:
              lifecycler:
                ring:
                  kvstore:
                    store: inmemory
                  replication_factor: 1
              chunk_idle_period: 5m
              chunk_retain_period: 30s
              wal:
                enabled: true
                dir: ${config.devenv.state}/loki/wal

            schema_config:
              configs:
                - from: 2024-01-01
                  store: tsdb
                  object_store: filesystem
                  schema: v13
                  index:
                    prefix: index_
                    period: 24h

            storage_config:
              tsdb_shipper:
                active_index_directory: ${config.devenv.state}/loki/index
                cache_location: ${config.devenv.state}/loki/cache
              filesystem:
                directory: ${config.devenv.state}/loki/chunks

            compactor:
              working_directory: ${config.devenv.state}/loki/compactor
              compaction_interval: 10m

            limits_config:
              retention_period: 1h
              max_query_lookback: 1h
          ''}
      '';
    };

    pyroscope = {
      ports = {
        http.allocate = 4040;
        # dskit binds gRPC unconditionally; without an allocated port two
        # worktrees collide on the 9095 default and the second one gives up.
        # The raft metastore (localhost:9099) and gossip port (7946) are other
        # fixed defaults two worktrees collide on.
        grpc.allocate = 9095;
        raft.allocate = 9099;
        memberlist.allocate = 7946;
      };

      # /ready reports real component health: the metastore, segment-writer and
      # ingester min-ready waits are disabled (single instance) and the raft
      # state is wiped on start (ports move), so it turns 200 within seconds
      # and fails when the metastore does.
      ready = {
        initial_delay = 3;
        period = 5;
        probe_timeout = 3;
        failure_threshold = 10;
        http.get = {
          port = pyroscopePorts.http.value;
          path = "/ready";
        };
      };

      shutdown.grace = 20;

      exec = ''
        # The single-node raft stores its voter config in this directory
        # including the bound port; once the allocator moves the port the
        # persisted config can never match again and no leader is elected.
        # Wipe the whole metastore (raft config + index data): partial state
        # mismatches panic the compaction service. Dev-only state — the
        # profile index is rebuilt as the emulator pushes.
        rm -rf ${config.devenv.state}/pyroscope/metastore
        mkdir -p ${config.devenv.state}/pyroscope/{data,data-compactor}
        mkdir -p ${config.devenv.state}/pyroscope/metastore/{data,raft}
        mkdir -p ${config.devenv.state}/pyroscope-sync

        exec ${pkgs.pyroscope}/bin/pyroscope \
          -config.file=${pkgs.writeText "pyroscope.yaml" ''
            server:
              http_listen_port: ${toString pyroscopePorts.http.value}
              grpc_listen_port: ${toString pyroscopePorts.grpc.value}

            metastore:
              # The metastore client dials this address; it must track the
              # moved gRPC port, not the 9095 default.
              address: 127.0.0.1:${toString pyroscopePorts.grpc.value}
              raft:
                server_id: localhost:${toString pyroscopePorts.raft.value}
                bind_address: localhost:${toString pyroscopePorts.raft.value}
                advertise_address: localhost:${toString pyroscopePorts.raft.value}

            storage:
              backend: filesystem
              filesystem:
                dir: ${config.devenv.state}/pyroscope

            pyroscopedb:
              data_path: ${config.devenv.state}/pyroscope/data

            compactor:
              data_dir: ${config.devenv.state}/pyroscope/data-compactor

            memberlist:
              bind_addr:
                - 127.0.0.1
              bind_port: ${toString pyroscopePorts.memberlist.value}
              abort_if_cluster_join_fails: false
          ''} \
          -blocks-storage.bucket-store.sync-dir=${config.devenv.state}/pyroscope-sync \
          -metastore.data-dir=${config.devenv.state}/pyroscope/metastore/data \
          -metastore.raft.dir=${config.devenv.state}/pyroscope/metastore/raft \
          -metastore.min-ready-duration=0 \
          -segment-writer.min-ready-duration=0 \
          -ingester.min-ready-duration=0 \
          -target=all \
          -self-profiling.disable-push=true
      '';
    };


    grafana = {
      ports.http.allocate = 3000;

      ready = {
        initial_delay = 3;
        probe_timeout = 5;
        failure_threshold = 10;
        http.get = {
          port = grafanaPorts.http.value;
          path = "/api/health";
        };
      };

      after = [
        "devenv:processes:tempo"
        "devenv:processes:loki"
        "devenv:processes:prometheus"
        "devenv:processes:pyroscope"
      ];

      exec = ''
        mkdir -p ${config.devenv.state}/grafana/data
        mkdir -p ${config.devenv.state}/grafana/logs
        mkdir -p ${config.devenv.state}/grafana/plugins
        mkdir -p ${config.devenv.state}/grafana/provisioning/datasources

        rm -f ${config.devenv.state}/grafana/provisioning/datasources/datasources.yaml
        cat ${grafanaDatasources} > ${config.devenv.state}/grafana/provisioning/datasources/datasources.yaml
        chmod 644 ${config.devenv.state}/grafana/provisioning/datasources/datasources.yaml

        export GF_PATHS_DATA=${config.devenv.state}/grafana/data
        export GF_PATHS_LOGS=${config.devenv.state}/grafana/logs
        export GF_PATHS_PLUGINS=${config.devenv.state}/grafana/plugins
        export GF_PATHS_PROVISIONING=${config.devenv.state}/grafana/provisioning
        export GF_SERVER_HTTP_PORT=${toString grafanaPorts.http.value}
        export GF_AUTH_ANONYMOUS_ENABLED=true
        export GF_AUTH_ANONYMOUS_ORG_ROLE=Admin
        export GF_SECURITY_ADMIN_PASSWORD=admin

        exec ${pkgs.grafana}/bin/grafana server \
          --homepath ${pkgs.grafana}/share/grafana \
          --config ${pkgs.grafana}/share/grafana/conf/defaults.ini
      '';
    };

    website = {
      cwd = "${config.devenv.root}/website";
      ports.http.allocate = 3001;

      ready = {
        initial_delay = 10;
        failure_threshold = 10;
        http.get = {
          port = websitePorts.http.value;
          path = "/";
        };
      };

      after = [ "website:install" ];

      exec = "exec npm run dev -- --port ${toString websitePorts.http.value}";
    };

    # `npm run storybook` pins `-p 6006`, so pass the allocated port to the
    # binary directly after running its locale prerequisite.
    storybook = {
      cwd = "${config.devenv.root}/website";
      ports.http.allocate = 6006;

      ready = {
        initial_delay = 15;
        failure_threshold = 20;
        timeout = 300;
        http.get = {
          port = storybookPorts.http.value;
          path = "/";
        };
      };

      after = [ "website:install" ];

      exec = ''
        npm run prestorybook
        exec ./node_modules/.bin/storybook dev -p ${toString storybookPorts.http.value}
      '';
    };

    mdbook = {
      cwd = "${config.devenv.root}/docs";
      ports.http.allocate = 3002;

      ready = {
        initial_delay = 5;
        failure_threshold = 10;
        http.get = {
          port = mdbookPorts.http.value;
          path = "/";
        };
      };

      exec = "exec mdbook serve --hostname 0.0.0.0 --port ${toString mdbookPorts.http.value}";
    };
  };

  # Tasks for building components with proper dependencies
  tasks = {
    # Build documentation EPUB (required for embedded assets)
    # Only rebuilds when docs files have changed (tracked via content hash)
    "docs:build" = {
      exec = "cadmuscli docs --mdbook-only";
      execIfModified = [
        "docs/**/*.md"
        "docs/book.toml"
        "docs/book"
        "docs/po"
      ];
    };

    # Build for Kobo with cross-compilation
    "build:kobo" = {
      exec = "cadmuscli build-kobo";
      after = [ "docs:build" ];
    };

    # Install website npm dependencies (only when package files change)
    "website:install" = {
      exec = "cd ${config.devenv.root}/website && npm install";
      execIfModified = [
        "website/package.json"
        "website/package-lock.json"
      ];
      before = [ "devenv:enterShell" ];
    };

    # Install OpenCode plugin tooling when package files or node_modules change.
    "opencode:install" = {
      exec = "cd ${config.devenv.root}/.opencode && npm install";
      execIfModified = [
        ".opencode/package.json"
        ".opencode/package-lock.json"
        ".opencode/node_modules"
      ];
      before = [ "devenv:enterShell" ];
    };

    # Release cadmuscli on PATH; rebuild when xtask sources change.
    "cadmuscli:install" = {
      exec = ''
        cargo build -p xtask --release
        target_dir="''${CARGO_TARGET_DIR:-$DEVENV_ROOT/target}"
        case "$target_dir" in
        /*) ;;
        *) target_dir="$DEVENV_ROOT/$target_dir" ;;
        esac
        mkdir -p "$DEVENV_STATE/bin" "$DEVENV_STATE/completions"
        ln -sfn "$target_dir/release/cadmuscli" "$DEVENV_STATE/bin/cadmuscli"
        test -e "$DEVENV_STATE/bin/cadmuscli"
        "$DEVENV_STATE/bin/cadmuscli" completions zsh --output-dir "$DEVENV_STATE/completions"
        "$DEVENV_STATE/bin/cadmuscli" completions bash --output-dir "$DEVENV_STATE/completions"
      '';
      execIfModified = [
        "xtask/**"
        "crates/build-deps/**"
        "Cargo.lock"
        "Cargo.toml"
        "${config.env.DEVENV_STATE}/bin/cadmuscli"
      ];
      before = [
        "devenv:enterShell"
        "docs:build"
      ];
    };

  };

  # Scripts are simple aliases that invoke cadmuscli commands
  scripts = {
    # Build complete documentation website (mdBook + Cargo docs + Next.js)
    cadmus-docs-build.exec = ''
      cadmuscli docs
    '';

    # `devenv up` supervises these (readiness, restart on crash); running the
    # dev command by hand gets neither.
    cadmus-docs-serve.exec = ''
      exec devenv up website
    '';

    cadmus-storybook.exec = ''
      exec devenv up storybook
    '';

    cadmus-docs-mdbook.exec = ''
      exec devenv up mdbook
    '';

    # `--wait` blocks for readiness; the script fails unless every process is
    # up, so it can gate anything that needs the stack.
    cadmus-process-status.exec = ''
      set -e

      if [ "''${1:-}" = "--wait" ]; then
        devenv processes wait --timeout 120
      fi

      status=0

      echo "URLs:"
      for name in \
        OTEL_EXPORTER_OTLP_ENDPOINT PYROSCOPE_SERVER_URL LOKI_URL TEMPO_URL \
        PROMETHEUS_URL GRAFANA_URL WEBSITE_URL MDBOOK_URL STORYBOOK_URL; do
        printf '  %-26s %s\n' "$name" "$(printenv "$name" || true)"
      done
      echo ""

      echo "Processes:"
      for name in \
        opentelemetry-collector tempo loki prometheus pyroscope grafana \
        website storybook mdbook; do
        if detail=$(devenv processes status "$name" 2>&1); then
          phase=$(printf '%s\n' "$detail" | sed -n 's/^Phase: *//p')
          printf '  %-26s %s\n' "$name" "$phase"
          if [ "$phase" != "ready" ]; then
            status=1
          fi
        else
          printf '  %-26s not running (%s; logs: $DEVENV_STATE/logs/%s.stdout.log)\n' \
            "$name" "$detail" "$name"
          status=1
        fi
      done
      echo ""

      echo "Health:"
      check() {
        label=$1
        url=$2
        if curl -fsS --max-time 5 "$url" >/dev/null; then
          printf '  %-26s ok\n' "$label"
        else
          printf '  %-26s FAILED (%s; logs: $DEVENV_STATE/logs/%s.stdout.log)\n' \
            "$label" "$url" "$label"
          status=1
        fi
      }

      check opentelemetry-collector "http://127.0.0.1:${toString otelPorts.health.value}/"
      check tempo "$TEMPO_URL/ready"
      check loki "$LOKI_URL/ready"
      check prometheus "$PROMETHEUS_URL/-/ready"
      check pyroscope "$PYROSCOPE_SERVER_URL/ready"
      check grafana "$GRAFANA_URL/api/health"
      check website "$WEBSITE_URL/"
      check storybook "$STORYBOOK_URL/"
      check mdbook "$MDBOOK_URL/"

      exit "$status"
    '';

    # Full static export via Workers (matches CI). Run cadmuscli docs first.
    cadmus-docs-preview.exec = ''
      echo "Starting wrangler dev for website/out (Workers Static Assets)..."
      echo "Note: run 'cadmuscli docs' first so website/out exists."
      echo ""
      wrangler dev
    '';

    # Build for Kobo device
    cadmus-build-kobo.exec = ''
      cadmuscli build-kobo "$@"
      cadmuscli dist
    '';

    # Typecheck, test, and format-check the OpenCode plugins
    cadmus-opencode-check.exec = ''
      cd ${config.devenv.root}/.opencode
      npm run typecheck
      npm test
      npm run format
    '';

    # Inspect and recover the sccache daemon. `restart` clears a server left
    # behind by an sccache upgrade ("Mismatch of client/server versions?") and
    # applies changed server-side settings such as SCCACHE_CACHE_SIZE.
    #
    # `--stop-server` returns as soon as shutdown is requested, and a lone
    # `--stop-server` failure means no server was running. Repeat the request
    # until it can no longer reach the server, so a replacement never runs
    # alongside a draining one sharing the same cache directory.
    cadmus-sccache.exec = ''
      set -e

      case "''${1:-stats}" in
        stats) exec sccache --show-adv-stats ;;
        reset) exec sccache --zero-stats ;;
        restart)
          if sccache --stop-server >/dev/null 2>&1; then
            stopped=0
            for _ in $(seq 1 100); do
              if ! sccache --stop-server >/dev/null 2>&1; then
                stopped=1
                break
              fi
              sleep 0.1
            done
            if [ "$stopped" -ne 1 ]; then
              echo "cadmus-sccache: sccache still accepts requests after 10s; not starting a second server" >&2
              exit 1
            fi
          fi
          exec sccache --start-server
          ;;
        log) exec tail -n "''${2:-50}" -f "$SCCACHE_ERROR_LOG" ;;
        *)
          echo "usage: cadmus-sccache [stats|reset|restart|log [lines]]" >&2
          exit 2
          ;;
      esac
    '';

    # Run clippy filtered to lines changed relative to master
    cadmus-clippy.exec = ''
      cadmuscli clippy --github-report --diff-branch master "$@"
    '';

    # Run emulator with OTEL instrumentation
    cadmus-dev-otel.exec = ''
      set -e

      echo ""
      echo "Stack URLs and health: run 'cadmus-process-status'."
      echo ""

      if ! devenv processes wait --timeout 120; then
        echo "Observability stack is not ready. Start it with 'devenv up' in another terminal." >&2
        exit 1
      fi

      # `devenv processes wait` also treats never-started, exited and given-up
      # processes as settled, so gate on each process actually reporting ready.
      for name in \
        opentelemetry-collector tempo loki prometheus pyroscope grafana; do
        if ! detail=$(devenv processes status "$name" 2>&1); then
          echo "cadmus-dev-otel: $name is not running ($detail; logs: $DEVENV_STATE/logs/$name.stdout.log)." >&2
          exit 1
        fi
        phase=$(printf '%s\n' "$detail" | sed -n 's/^Phase: *//p')
        if [ "$phase" != "ready" ]; then
          echo "cadmus-dev-otel: $name is ''${phase:-unknown} (logs: $DEVENV_STATE/logs/$name.stdout.log)." >&2
          exit 1
        fi
      done
      echo ""

      echo "Ensuring documentation is built..."
      devenv tasks run docs:build
      echo ""

      echo "Starting instrumented emulator..."
      echo "   Traces will be visible in Grafana → Explore → Tempo"
      echo "   Heap profiles will be visible in Grafana → Explore → Pyroscope"
      echo ""

      cadmuscli run-emulator --features telemetry,test
    '';

    # Extract translatable strings from documentation
    cadmus-translate.exec = ''
      echo "Extracting translatable strings from documentation..."
      MDBOOK_OUTPUT='{"xgettext": {}}' mdbook build -d $DEVENV_ROOT/docs/po $DEVENV_ROOT/docs
      echo ""
      echo "Updated docs/po/messages.pot"
      echo "Translate on Crowdin: https://crowdin.com/project/cadmus"
    '';

    # Run instrumented tests; writes target/coverage/lcov.info
    cadmus-test-coverage.exec = ''
      cd "$DEVENV_ROOT"
      if [ $# -eq 0 ]; then
        set -- --features emulator
      fi
      cadmuscli test --coverage "$@"
    '';

    # Project-wide HTML from last coverage run (no re-test)
    cadmus-coverage-show.exec = ''
      set -e
      cd "$DEVENV_ROOT"
      if ! cargo llvm-cov report --html \
        --ignore-filename-regex '(thirdparty/|mupdf_wrapper/|build\.rs)'; then
        echo "No coverage data. Run 'cadmus-test-coverage' first." >&2
        exit 1
      fi
      ${openHtml "target/llvm-cov/html/index.html"}
    '';

    # Patch HTML vs base branch (no re-test). Branch from $1 or $CADMUS_DEFAULT_BRANCH.
    cadmus-coverage-diff.exec = ''
      set -e
      cd "$DEVENV_ROOT"
      BRANCH="''${1:-$CADMUS_DEFAULT_BRANCH}"
      LCOV="target/coverage/lcov.info"
      if [ ! -f "$LCOV" ]; then
        echo "No coverage data. Run 'cadmus-test-coverage' first." >&2
        exit 1
      fi
      case "$BRANCH" in
        origin/*) COMPARE_BRANCH="$BRANCH" ;;
        *) COMPARE_BRANCH="origin/$BRANCH" ;;
      esac
      mkdir -p target/coverage
      diff-cover "$LCOV" \
        --compare-branch "$COMPARE_BRANCH" \
        --format "html:target/coverage/diff-coverage.html"
      ${openHtml "target/coverage/diff-coverage.html"}
    '';
  };

  enterShell = ''
    echo "Cadmus development environment"
    echo ""
    echo "Available commands:"
    echo "  cadmus-docs-build     - Build complete documentation portal"
    echo "  cadmus-docs-preview   - wrangler dev for website/out (Workers, matches CI)"
    echo "  cadmus-docs-serve     - Serve website locally ($WEBSITE_URL)"
    echo "  cadmus-translate      - Regenerate docs/po/messages.pot (translations on Crowdin)"
    echo "  cadmuscli run-emulator - Run the emulator (after setup)"
    echo "  cargo test            - Run tests (after setup)"
    echo ""
    echo "Coverage (diff base: $CADMUS_DEFAULT_BRANCH):"
    echo "  cadmus-test-coverage  - Run tests with llvm-cov instrumentation"
    echo "  cadmus-coverage-show  - Project-wide HTML report (after test-coverage)"
    echo "  cadmus-coverage-diff  - Patch HTML vs origin/\$CADMUS_DEFAULT_BRANCH or [branch] arg"
    echo ""
    echo "cadmuscli commands (cadmuscli <cmd> --help for options):"
    echo "  cadmus-clippy             - Lint lines changed vs master (reviewdog)"
    echo "  cadmuscli bundle          - Package KoboRoot.tgz"
    echo "  cadmuscli clippy          - Lint across feature matrix"
    echo "  cadmuscli dist            - Assemble Kobo distribution"
    echo "  cadmuscli docs            - Build documentation portal"
    echo "  cadmuscli fmt             - Check formatting"
    echo "  cadmuscli test            - Test across feature matrix"
    echo "  cargo xtask still works as a debug cargo run alias."
    echo ""
  ''
  + ''
    export PATH="${config.env.DEVENV_STATE}/bin:${linaroToolchain}/bin:$PATH"

    _cadmuscli_completions="${config.env.DEVENV_STATE}/completions"
    if [ -d "$_cadmuscli_completions" ]; then
      if [ -n "''${BASH_VERSION:-}" ] && [ -f "$_cadmuscli_completions/cadmuscli.bash" ]; then
        . "$_cadmuscli_completions/cadmuscli.bash"
      fi
      _cadmuscli_zshrc="${config.env.DEVENV_DOTFILE}/zsh/.zshrc"
      _cadmuscli_zsh_marker="# cadmuscli shell completions"
      if [ -f "$_cadmuscli_zshrc" ] && ! grep -q "$_cadmuscli_zsh_marker" "$_cadmuscli_zshrc"; then
        printf '%s\n' \
          "" \
          '# cadmuscli shell completions' \
          'if [[ -n "$DEVENV_STATE" && -f "$DEVENV_STATE/completions/_cadmuscli" ]]; then' \
          '  fpath=("$DEVENV_STATE/completions" $fpath)' \
          '  autoload -Uz compinit' \
          '  compinit -C' \
          '  autoload -Uz _cadmuscli' \
          '  compdef _cadmuscli cadmuscli' \
          'fi' \
          >> "$_cadmuscli_zshrc"
      fi
    fi

    echo "Cross-compilation:"
    echo "  cadmus-build-kobo         - Build for Kobo (cross-compile + dist)"
    echo "  cadmuscli build-kobo      - Cross-compile Cadmus for Kobo"
    echo "  Linaro toolchain: $(which arm-linux-gnueabihf-gcc 2>/dev/null || echo 'not found')"
    echo ""
  ''
  + ''
    echo "Observability (OTEL):"
    echo "  devenv up                - Start all services (inc. observability stack)"
    echo "  cadmus-process-status    - Print URLs, process state and health"
    echo "  cadmus-dev-otel          - Build & run emulator with OTEL enabled"
    echo ""
    echo "  Grafana:    $GRAFANA_URL (admin/admin)"
    echo "  Tempo:      $TEMPO_URL"
    echo "  Loki:       $LOKI_URL"
    echo "  Prometheus: $PROMETHEUS_URL"
    echo "  Pyroscope:  $PYROSCOPE_SERVER_URL"
    echo "  OTLP:       $OTEL_EXPORTER_OTLP_ENDPOINT"
    echo ""
    echo "  Ports are allocated: with a manager running, 'devenv up' and this"
    echo "  shell print the same resolved ports; otherwise the base ports apply."
    echo ""
    echo "Docs site:"
    echo "  cadmus-docs-serve        - Next dev server ($WEBSITE_URL)"
    echo "  cadmus-storybook         - Storybook ($STORYBOOK_URL)"
    echo "  cadmus-docs-mdbook       - mdBook preview ($MDBOOK_URL)"
    echo ""
    echo "Build cache (sccache, incremental compilation disabled):"
    echo "  cadmus-sccache        - stats|reset|restart|log"
    echo ""
  ''
  # devenv leaves `RUST_SRC_PATH` unset when the toolchain has its own sysroot
  # and no known source path; nothing to link in that case.
  + pkgs.lib.optionalString (config.env.RUST_SRC_PATH != null) ''
    echo "Linking rust source for stable access"
    # rm first: `ln -fs` follows an existing symlink-to-directory and tries to
    # create the link inside it, which fails on the read-only nix store.
    rm -f ${config.env.DEVENV_STATE}/rust-lib-src
    ln -s ${config.env.RUST_SRC_PATH} ${config.env.DEVENV_STATE}/rust-lib-src
  '';

  # https://devenv.sh/tests/
  enterTest = ''
    set -e
    export PATH="${config.env.DEVENV_STATE}/bin:$PATH"
    cadmuscli test --features emulator
    cadmuscli test --features kobo
  '';

  treefmt = {
    enable = true;
    config = {
      programs = {
        prettier.enable = true;
        rustfmt.enable = true;
        shellcheck.enable = true;
        shfmt.enable = true;
        yamllint = {
          enable = true;
          settings.extends = "default";
          settings.rules = {
            line-length = "disable";
            comments = "disable";
            document-start = "disable";
            new-line-at-end-of-file = "disable";
            truthy = "disable";
          };
        };
        rumdl-check.enable = true;
      };

      settings = {
        excludes = [
          ".sqlx/**"
          "doc/**"
        ];
        formatter = {
          # treefmt-nix already supplies -w, -i (via programs.shfmt.indent_size),
          # -s (via programs.shfmt.simplify) and the file globs. Formatter
          # settings are open submodules that merge rather than replace, so
          # this only adds what the module has no option for: CI's -ci case
          # indent. Repeating the other flags here just duplicates them.
          shfmt = {
            options = [ "-ci" ];
          };

          # actionlint does not support ignore patterns in its config file;
          # the -ignore flag must be passed on the command line, so we define
          # the formatter manually. Patterns live in
          # .github/lint/actionlint-ignore.txt, shared with CI (see
          # actions-lint.yml), rather than being duplicated here.
          actionlint = {
            command = "${pkgs.actionlint}/bin/actionlint";
            options = pkgs.lib.concatMap (pattern: [
              "-ignore"
              pattern
            ]) actionlintIgnorePatterns;
            includes = [
              ".github/workflows/*.yml"
              ".github/workflows/*.yaml"
            ];
          };
        };
      };
    };
  };

  git-hooks.hooks = {
    treefmt.enable = true;
    coderabbit-schema = {
      enable = true;
      name = "CodeRabbit schema";
      files = "^\\.coderabbit\\.yaml$";
      entry = "${pkgs.check-jsonschema}/bin/check-jsonschema --schemafile https://coderabbit.ai/integrations/schema.v2.json .coderabbit.yaml";
      pass_filenames = false;
    };
    eslint = {
      enable = true;
      name = "eslint";
      entry = "sh -c 'cd website && ./node_modules/.bin/eslint .'";
      files = "^website/.*\\.(ts|tsx|mjs)$";
      language = "system";
      pass_filenames = false;
    };
    stylelint = {
      enable = true;
      name = "stylelint";
      entry = "sh -c 'cd website && ./node_modules/.bin/stylelint \"**/*.css\"'";
      files = "^website/.*\\.css$";
      language = "system";
      pass_filenames = false;
    };
  };
}
