{
  pkgs,
  config,
  ...
}:

let
  # Linaro GCC toolchain for Kobo - same as used by Kobo Reader
  # https://github.com/kobolabs/Kobo-Reader/blob/master/toolchain/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz
  linaroToolchain = pkgs.stdenv.mkDerivation {
    pname = "gcc-linaro";
    version = "4.9.4-2017.01";

    src = pkgs.fetchurl {
      url = "https://releases.linaro.org/components/toolchain/binaries/4.9-2017.01/arm-linux-gnueabihf/gcc-linaro-4.9.4-2017.01-x86_64_arm-linux-gnueabihf.tar.xz";
      sha256 = "22914118fd963f953824b58107015c6953b5bbdccbdcf25ad9fd9a2f9f11ac07";
    };

    nativeBuildInputs = [ pkgs.autoPatchelfHook ];
    buildInputs = [
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

    # The toolchain has pre-built binaries that need patching
    # Ignore python dependency for gdb (we don't need gdb for building)
    autoPatchelfIgnoreMissingDeps = [ "libpython2.7.so.1.0" ];
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
  '';
in
{
  overlays = [ ];

  packages = [
    # Basic tools required by build scripts
    pkgs.git
    pkgs.wget
    pkgs.curl
    pkgs.pkg-config
    pkgs.unzip
    pkgs.jq
    pkgs.patchelf

    pkgs.mdbook
    mdbook-epub-custom

    # C/C++ build tools for compiling thirdparty libraries
    pkgs.gcc
    pkgs.gnumake
    pkgs.cmake
    pkgs.meson
    pkgs.ninja
    pkgs.autoconf
    pkgs.automake
    pkgs.libtool
    pkgs.gperf
    pkgs.python3

    # Linaro ARM cross-compilation toolchain (provides arm-linux-gnueabihf-* commands)
    linaroToolchain

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
  ];

  # Enable Rust with cross-compilation support
  languages = {
    javascript = {
      enable = true;
      npm = {
        enable = true;
      };
    };
    rust = {
      enable = true;
      channel = "stable";
      targets = [ "arm-unknown-linux-gnueabihf" ];
    };
  };

  env = {
    # override this in devenv.local.nix to the right place for your test cadmus root dir
    # TEST_ROOT_DIR = "$DEVENV_ROOT" ;

    # pkg-config configuration for cross-compilation
    PKG_CONFIG_ALLOW_CROSS = "1";

    # Cargo linker for ARM target (only used when building for ARM)
    CARGO_TARGET_ARM_UNKNOWN_LINUX_GNUEABIHF_LINKER = "arm-linux-gnueabihf-gcc";

    # C compiler for ARM target (used by cc crate for build scripts)
    CC_arm_unknown_linux_gnueabihf = "arm-linux-gnueabihf-gcc";
    AR_arm_unknown_linux_gnueabihf = "arm-linux-gnueabihf-ar";

    RUST_LOG = "debug";
    RUST_BACKTRACE = "1";
    OTEL_EXPORTER_OTLP_ENDPOINT = "http://localhost:4318";
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

            query_frontend:
              search:
                max_duration: 1h
          ''} \
          -target=all \
          -backend-scheduler.local-work-path=${config.devenv.state}/tempo/work
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
              grpc_listen_port: 9095

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

  scripts = {
    # Script to build mupdf for native development
    cadmus-setup-native.exec = ''
      set -e
      echo "Setting up native development environment..."

      # Check mupdf version and re-download if needed
      REQUIRED_MUPDF_VERSION="1.27.0"
      CURRENT_MUPDF_VERSION=""
      if [ -e thirdparty/mupdf/include/mupdf/fitz/version.h ]; then
        CURRENT_MUPDF_VERSION=$(grep -o 'FZ_VERSION "[^"]*"' thirdparty/mupdf/include/mupdf/fitz/version.h | grep -o '"[^"]*"' | tr -d '"')
      fi

      if [ "$CURRENT_MUPDF_VERSION" != "$REQUIRED_MUPDF_VERSION" ]; then
        echo "MuPDF version mismatch: have '$CURRENT_MUPDF_VERSION', need '$REQUIRED_MUPDF_VERSION'"
        echo "Downloading mupdf $REQUIRED_MUPDF_VERSION sources..."
        # Remove old mupdf and re-download
        rm -rf thirdparty/mupdf
        cd thirdparty
        ./download.sh mupdf
        cd ..
      else
        echo "MuPDF $CURRENT_MUPDF_VERSION already present."
      fi

      # Build mupdf wrapper for Linux
      echo "Building mupdf wrapper..."
      cd mupdf_wrapper
      ./build.sh
      cd ..

      # Build MuPDF for native development using system libraries from Nix
      # We skip building cadmus/thirdparty/* and use pkg-config to find system libs
      echo "Building mupdf for native development..."
      cd thirdparty/mupdf
      [ -e .gitattributes ] && rm -rf .git*

      # Clean any previous builds
      make clean || true

      # Generate sources
      make verbose=yes generate

      # Build MuPDF libraries using system libraries (detected via pkg-config)
      make verbose=yes \
        mujs=no tesseract=no extract=no archive=no brotli=no barcode=no commercial=no \
        USE_SYSTEM_LIBS=yes \
        XCFLAGS="-DFZ_ENABLE_ICC=0 -DFZ_ENABLE_SPOT_RENDERING=0 -DFZ_ENABLE_ODT_OUTPUT=0 -DFZ_ENABLE_OCR_OUTPUT=0" \
        libs

      cd ../..

      # Create target directory structure
      mkdir -p target/mupdf_wrapper/Linux

      # Copy/link libmupdf.a
      if [ -e thirdparty/mupdf/build/release/libmupdf.a ]; then
        ln -sf "$(pwd)/thirdparty/mupdf/build/release/libmupdf.a" target/mupdf_wrapper/Linux/
        echo "✓ Created libmupdf.a"
      else
        echo "✗ ERROR: libmupdf.a not found!"
        exit 1
      fi

      # When using USE_SYSTEM_LIBS=yes, MuPDF doesn't create libmupdf-third.a
      # because dependencies come from system libraries via pkg-config.
      # Create an empty libmupdf-third.a to satisfy cargo's build requirements.
      if [ ! -e thirdparty/mupdf/build/release/libmupdf-third.a ]; then
        echo "Creating empty libmupdf-third.a (system libs used instead)..."
        ar cr thirdparty/mupdf/build/release/libmupdf-third.a
      fi
      ln -sf "$(pwd)/thirdparty/mupdf/build/release/libmupdf-third.a" target/mupdf_wrapper/Linux/
      echo "✓ Created libmupdf-third.a"

      echo ""
      echo "Native setup complete! You can now run:"
      echo "  cargo test          - Run tests"
      echo "  ./run-emulator.sh   - Run the emulator"
    '';

    # Script to build for Kobo with proper cross-compilation environment
    cadmus-build-kobo.exec = ''
      set -e

      # Set up cross-compilation environment
      export CC=arm-linux-gnueabihf-gcc
      export CXX=arm-linux-gnueabihf-g++
      export AR=arm-linux-gnueabihf-ar
      export LD=arm-linux-gnueabihf-ld
      export RANLIB=arm-linux-gnueabihf-ranlib
      export STRIP=arm-linux-gnueabihf-strip

      # Run the build script
      exec ./build.sh "$@"
      exec ./dist.sh
    '';

    # Build and run emulator with OTEL instrumentation
    cadmus-dev-otel.exec = ''
      set -e

      echo ""
      echo "Observability Stack:"
      echo "  Grafana:    http://localhost:3000 (admin/admin)"
      echo "  Tempo:      http://localhost:3200"
      echo "  Loki:       http://localhost:3100"
      echo "  Prometheus: http://localhost:9090"
      echo "  OTLP:       http://localhost:4318"
      echo ""
      echo "Starting instrumented emulator..."
      echo "   Traces will be visible in Grafana → Explore → Tempo"
      echo ""

      export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4318"

      ./run-emulator.sh --features otel "$@"
    '';
  };

  enterShell = ''
    # Add Linaro toolchain to PATH
    export PATH="${linaroToolchain}/bin:$PATH"

    echo "Cadmus development environment"
    echo ""
    echo "Available commands:"
    echo "  cadmus-setup-native  - Build mupdf for native development (run once)"
    echo "  cadmus-build-kobo    - Build for Kobo (sets up cross-compilation env)"
    echo "  cargo test           - Run tests (after setup)"
    echo "  ./run-emulator.sh    - Run the emulator (after setup)"
    echo ""
    echo "Observability (OTEL):"
    echo "  devenv up            - Start all services (inc. observability stack)"
    echo "  cadmus-dev-otel      - Build & run emulator with OTEL enabled"
    echo ""
    echo "  After 'devenv up', visit http://localhost:3000 for Grafana"
    echo ""
    echo "Linaro toolchain: $(which arm-linux-gnueabihf-gcc 2>/dev/null || echo 'not found')"
  '';

  # https://devenv.sh/tests/
  enterTest = ''
    echo "Running Cadmus tests"
    cargo test --workspace
  '';

  git-hooks.hooks = {
    actionlint.enable = true;
    shellcheck.enable = true;
    shfmt.enable = true;
    markdownlint.enable = true;
    prettier.enable = true;
  };
}
