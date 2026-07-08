{
  pkgs,
  config,
  inputs,
  ...
}:

let
  inherit (pkgs.stdenv) isLinux;
  inherit (pkgs.stdenv) isDarwin;

  # Rust 1.94+ changed lib/ in tarballs from a directory to a symlink.
  # devenv's rust module calls mk-aggregated.nix directly from the rust-overlay
  # source, bypassing pkgs overlays. lndir can't merge components when $out/lib
  # is already a symlink, so cp --remove-destination in postBuild fails with
  # "are the same file" on macOS.
  #
  # Fix: wrap the produced toolchain derivation to resolve $out/lib from a
  # symlink into a real directory before the cp step runs.
  fixRustToolchainLibSymlink =
    drv:
    drv.overrideAttrs (old: {
      buildCommand =
        builtins.replaceStrings
          [ "for i in $(cat $pathsPath); do\n" ]
          [
            (
              "for i in $(cat $pathsPath); do\n"
              + "  if [ -L \"$out/lib\" ]; then\n"
              + "    _lib_t=$(readlink -f \"$out/lib\")\n"
              + "    rm \"$out/lib\"\n"
              + "    mkdir \"$out/lib\"\n"
              + "    ${pkgs.lndir}/bin/lndir -silent \"$_lib_t\" \"$out/lib\"\n"
              + "  fi\n"
            )
          ]
          old.buildCommand;
    });

  # Build the stable Rust toolchain the same way devenv does, but with the
  # lib-symlink fix applied. We replicate devenv's channel != "nixpkgs" logic
  # here so we can wrap the resulting derivation before devenv sees it.
  rustToolchain =
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

      toolchain = rustBin.stable.latest;
      nativeTarget = pkgs.stdenv.hostPlatform.rust.rustcTargetSpec;
      allTargets = pkgs.lib.unique ([ nativeTarget ] ++ [ "arm-unknown-linux-gnueabihf" ]);
      components = [
        "cargo"
        "clippy"
        "llvm-tools-preview"
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
    fixRustToolchainLibSymlink profile;

  # cargo-diff-tools with Rust 1.70 for clap v2 compatibility
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

  # Grafana datasource provisioning
  grafanaDatasources = pkgs.writeText "datasources.yaml" ''
    apiVersion: 1
    datasources:
      - name: Tempo
        type: tempo
        access: proxy
        url: http://localhost:3200
        isDefault: false
        editable: true

      - name: Loki
        type: loki
        access: proxy
        url: http://localhost:3100
        isDefault: false
        editable: true

      - name: Prometheus
        type: prometheus
        access: proxy
        url: http://localhost:9090
        isDefault: true
        editable: true

      - name: Pyroscope
        type: grafana-pyroscope-datasource
        access: proxy
        url: http://localhost:4040
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
    pkgs.SDL2

    # Native build dependencies (development headers)
    pkgs.zlib
    pkgs.bzip2
    pkgs.libpng
    pkgs.libjpeg
    pkgs.openjpeg
    pkgs.jbig2dec
    pkgs.gumbo

    # Observability tools (for OTEL instrumentation in dev mode)
    pkgs.grafana
    pkgs.tempo
    pkgs.grafana-loki
    pkgs.pyroscope

    # SQLx CLI for database migrations and compile-time query verification
    pkgs.sqlx-cli
    pkgs.sqlite
    pkgs.sqlitebrowser

    pkgs.wrangler

    pkgs.cargo-llvm-cov

    # Linaro ARM cross-compilation toolchain (provides arm-linux-gnueabihf-* commands)
    linaroToolchain

    # patchelf is used to patch ELF binaries
    pkgs.patchelf
  ]
  # Linux-only packages
  ++ pkgs.lib.optionals isLinux [
    # GCC - on macOS we use clang from Xcode
    pkgs.gcc

    # This seems to be borken on macos
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
      targets = [ "arm-unknown-linux-gnueabihf" ];
      toolchain = {
        inherit (pkgs) cargo-expand;
      };
      components = [
        "cargo"
        "clippy"
        "llvm-tools-preview"
        "rustc"
        "rustfmt"
      ];
      toolchainPackage = pkgs.lib.mkForce rustToolchain;
    };
  };

  dotenv.enable = true;

  env = {
    # override this in devenv.local.nix to the right place for your test cadmus root dir
    # TEST_ROOT_DIR = "$DEVENV_ROOT" ;

    RUST_LOG = "emulator=debug,cadmus_core=debug";
    RUST_BACKTRACE = "1";
    OTEL_EXPORTER_OTLP_ENDPOINT = "http://localhost:4318";
    PYROSCOPE_SERVER_URL = "http://localhost:4040";
    NEXTEST_NO_TESTS = "pass";

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

    # Point libsqlite3-sys at the custom SQLite build (cargo xtask setup).
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
  };

  services.opentelemetry-collector = {
    enable = true;
    settings = {
      receivers.otlp.protocols = {
        grpc.endpoint = "0.0.0.0:4317";
        http.endpoint = "0.0.0.0:4318";
      };

      processors.batch = { };

      exporters = {
        "otlp/tempo" = {
          endpoint = "localhost:4327";
          tls.insecure = true;
        };

        loki = {
          endpoint = "http://localhost:3100/loki/api/v1/push";
        };

        prometheus = {
          endpoint = "0.0.0.0:8889";
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
          exporters = [ "loki" ];
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
            targets = [ "localhost:8889" ];
          }
        ];
      }
      {
        job_name = "prometheus";
        static_configs = [
          {
            targets = [ "localhost:9090" ];
          }
        ];
      }
    ];
  };

  # Processes for Grafana, Tempo, and Loki
  processes = {
    tempo = {
      exec = ''
        mkdir -p ${config.devenv.state}/tempo/{traces,wal,work}

        ${pkgs.tempo}/bin/tempo \
          -config.file=${pkgs.writeText "tempo.yaml" ''
            server:
              http_listen_port: 3200
              grpc_listen_port: 9096
              grpc_server_max_recv_msg_size: 104857600
              grpc_server_max_send_msg_size: 104857600

            distributor:
              receivers:
                otlp:
                  protocols:
                    grpc:
                      endpoint: 0.0.0.0:4327
                    http:
                      endpoint: 0.0.0.0:4328

            ingester:
              max_block_duration: 5m
              trace_idle_period: 10s

            memberlist:
              bind_addr:
                - 127.0.0.1
              abort_if_cluster_join_fails: false

            compactor:
              compaction:
                block_retention: 1h
              ring:
                kvstore:
                  store: inmemory
                instance_addr: 127.0.0.1

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
      exec = ''
        mkdir -p ${config.devenv.state}/loki/{index,cache,chunks,wal,compactor}

        ${pkgs.grafana-loki}/bin/loki \
          -config.file=${pkgs.writeText "loki.yaml" ''
            auth_enabled: false

            server:
              http_listen_port: 3100
              grpc_listen_port: 9097

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
      exec = ''
        mkdir -p ${config.devenv.state}/pyroscope/{data,data-compactor}
        mkdir -p ${config.devenv.state}/pyroscope-sync

        ${pkgs.pyroscope}/bin/pyroscope \
          -config.file=${pkgs.writeText "pyroscope.yaml" ''
            server:
              http_listen_port: 4040

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
              abort_if_cluster_join_fails: false
          ''} \
          -blocks-storage.bucket-store.sync-dir=${config.devenv.state}/pyroscope-sync \
          -target=all \
          -self-profiling.disable-push=true
      '';
    };

    grafana = {
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
        export GF_SERVER_HTTP_PORT=3000
        export GF_AUTH_ANONYMOUS_ENABLED=true
        export GF_AUTH_ANONYMOUS_ORG_ROLE=Admin
        export GF_SECURITY_ADMIN_PASSWORD=admin

        ${pkgs.grafana}/bin/grafana server \
          --homepath ${pkgs.grafana}/share/grafana \
          --config ${pkgs.grafana}/share/grafana/conf/defaults.ini
      '';
    };
  };

  # Tasks for building components with proper dependencies
  tasks = {
    # Build documentation EPUB (required for embedded assets)
    # Only rebuilds when docs files have changed (tracked via content hash)
    "docs:build" = {
      exec = "cargo xtask docs --mdbook-only";
      execIfModified = [
        "docs/**/*.md"
        "docs/book.toml"
        "docs/book"
        "docs/po"
      ];
    };

    # Build for Kobo with cross-compilation
    "build:kobo" = {
      exec = "cargo xtask build-kobo";
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

  };

  # Scripts are simple aliases that invoke xtask commands
  scripts = {
    # Build complete documentation website (mdBook + Cargo docs + Next.js)
    cadmus-docs-build.exec = ''
      cargo xtask docs
    '';

    # Serve website locally (run cargo xtask docs --mdbook-only first to build mdBook)
    cadmus-docs-serve.exec = ''
      echo "Starting website dev server..."
      echo "Note: run 'cargo xtask docs --mdbook-only' first to build mdBook output."
      echo ""
      cd website
      npm run dev
    '';

    # Build for Kobo device
    cadmus-build-kobo.exec = ''
      cargo xtask build-kobo "$@"
      cargo xtask dist
    '';

    # Run clippy filtered to lines changed relative to master
    cadmus-clippy.exec = ''
      cargo xtask clippy --github-report --diff-branch master "$@"
    '';

    # Run emulator with OTEL instrumentation
    cadmus-dev-otel.exec = ''
      set -e

      echo ""
      echo "Observability Stack:"
      echo "  Grafana:    http://localhost:3000 (admin/admin)"
      echo "  Tempo:      http://localhost:3200"
      echo "  Loki:       http://localhost:3100"
      echo "  Prometheus: http://localhost:9090"
      echo "  Pyroscope:  http://localhost:4040"
      echo "  OTLP:       http://localhost:4318"
      echo ""

      echo "Ensuring documentation is built..."
      devenv tasks run docs:build
      echo ""

      echo "Starting instrumented emulator..."
      echo "   Traces will be visible in Grafana → Explore → Tempo"
      echo "   Heap profiles will be visible in Grafana → Explore → Pyroscope"
      echo ""

      export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4318"
      export PYROSCOPE_SERVER_URL="http://localhost:4040"
      export RUST_LOG="emulator=trace,cadmus_core=trace"
      cargo xtask run-emulator --features telemetry,test,emulator
    '';

    # Extract translatable strings from documentation
    cadmus-translate.exec = ''
      echo "Extracting translatable strings from documentation..."
      MDBOOK_OUTPUT='{"xgettext": {}}' mdbook build -d $DEVENV_ROOT/docs/po $DEVENV_ROOT/docs
      echo ""
      echo "Updated docs/po/messages.pot"
      echo "Translate on Crowdin: https://crowdin.com/project/cadmus"
    '';
  };

  enterShell = ''
    export RUSTDOCFLAGS="''${RUSTDOCFLAGS:+''$RUSTDOCFLAGS }-D warnings"

    echo "Cadmus development environment"
    echo ""
    echo "Available commands:"
    echo "  cadmus-docs-build     - Build complete documentation portal"
    echo "  cadmus-docs-serve     - Serve website locally (http://localhost:3000)"
    echo "  cargo test            - Run tests (after setup)"
    echo "  cargo xtask run-emulator - Run the emulator (after setup)"
    echo "  cadmus-translate      - Regenerate docs/po/messages.pot (translations on Crowdin)"
    echo ""
    echo "xtask commands (cargo xtask <cmd> --help for options):"
    echo "  cargo xtask fmt           - Check formatting"
    echo "  cadmus-clippy             - Lint lines changed vs master (reviewdog)"
    echo "  cargo xtask clippy        - Lint across feature matrix"
    echo "  cargo xtask test          - Test across feature matrix"
    echo "  cargo xtask docs          - Build documentation portal"
    echo "  cargo xtask dist          - Assemble Kobo distribution"
    echo "  cargo xtask bundle        - Package KoboRoot.tgz"
    echo ""
  ''
  + ''
    # Add Linaro toolchain to PATH
    export PATH="${linaroToolchain}/bin:$PATH"

    echo "Cross-compilation:"
    echo "  cadmus-build-kobo         - Build for Kobo (cross-compile + dist)"
    echo "  cargo xtask build-kobo    - Cross-compile Cadmus for Kobo"
    echo "  Linaro toolchain: $(which arm-linux-gnueabihf-gcc 2>/dev/null || echo 'not found')"
    echo ""
  ''
  + ''
    echo "Observability (OTEL):"
    echo "  devenv up            - Start all services (inc. observability stack)"
    echo "  cadmus-dev-otel      - Build & run emulator with OTEL enabled"
    echo ""
    echo "  After 'devenv up', visit http://localhost:3000 for Grafana"
    echo "  Pyroscope profiling UI: http://localhost:4040"
    echo ""

     echo "Linking rust source for stable access"
     ln -fs ${config.env.RUST_SRC_PATH} ${config.env.DEVENV_STATE}/rust-lib-src
  '';

  # https://devenv.sh/tests/
  enterTest = ''
    echo "Running Cadmus tests"
    cargo test --workspace
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
          # The treefmt-nix shfmt module does not expose a case-indent option,
          # so we override the formatter directly to match CI's -ci flag.
          shfmt = {
            command = "${pkgs.shfmt}/bin/shfmt";
            options = [
              "-i"
              "2"
              "-ci"
              "-w"
            ];
            includes = [
              "*.sh"
              "*.bash"
              "*.envrc"
              "*.envrc.*"
            ];
          };

          # actionlint does not support ignore patterns in its config file;
          # the -ignore flag must be passed on the command line. We define the
          # formatter manually so we can suppress the false positive that arises
          # from YAML anchors: the step ID 'rust-toolchain' is defined in every
          # job that expands the anchor but actionlint cannot resolve it
          # statically from the anchor body alone.
          actionlint = {
            command = "${pkgs.actionlint}/bin/actionlint";
            options = [
              "-ignore"
              ''"rust-toolchain" is not defined in object type''
            ];
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
    cargo-test = {
      enable = true;
      name = "cargo test (default features)";
      entry = "cargo test --workspace --features default";
      files = "\\.rs$";
      pass_filenames = false;
      language = "system";
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
