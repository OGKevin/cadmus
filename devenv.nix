{
  pkgs,
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
in
{
  packages = [
    # Basic tools required by build scripts
    pkgs.git
    pkgs.wget
    pkgs.curl
    pkgs.pkg-config
    pkgs.unzip
    pkgs.jq
    pkgs.patchelf

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
  ];

  # Enable Rust with cross-compilation support
  languages = {
    rust = {
      enable = true;
      channel = "stable";
      targets = [ "arm-unknown-linux-gnueabihf" ];
    };
  };

  env = {
    # pkg-config configuration for cross-compilation
    PKG_CONFIG_ALLOW_CROSS = "1";

    # Cargo linker for ARM target (only used when building for ARM)
    CARGO_TARGET_ARM_UNKNOWN_LINUX_GNUEABIHF_LINKER = "arm-linux-gnueabihf-gcc";

    # C compiler for ARM target (used by cc crate for build scripts)
    CC_arm_unknown_linux_gnueabihf = "arm-linux-gnueabihf-gcc";
    AR_arm_unknown_linux_gnueabihf = "arm-linux-gnueabihf-ar";
  };

  scripts = {
    # Script to build mupdf for native development
    plato-setup-native.exec = ''
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
      # We skip building plato/thirdparty/* and use pkg-config to find system libs
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
    plato-build-kobo.exec = ''
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
  };

  enterShell = ''
    # Add Linaro toolchain to PATH
    export PATH="${linaroToolchain}/bin:$PATH"

    echo "Plato development environment"
    echo ""
    echo "Available commands:"
    echo "  plato-setup-native  - Build mupdf for native development (run once)"
    echo "  plato-build-kobo    - Build for Kobo (sets up cross-compilation env)"
    echo "  cargo test          - Run tests (after setup)"
    echo "  ./run-emulator.sh   - Run the emulator (after setup)"
    echo ""
    echo "Linaro toolchain: $(which arm-linux-gnueabihf-gcc 2>/dev/null || echo 'not found')"
  '';

  # https://devenv.sh/tests/
  enterTest = ''
    echo "Running Plato tests"
    cargo test --workspace
  '';
}
