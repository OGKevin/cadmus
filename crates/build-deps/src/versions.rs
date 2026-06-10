//! Build-time constants that pin the thirdparty toolchain to specific
//! versions and document the layout each build flow expects on disk.
//!
//! With git submodules, individual library versions are tracked by
//! the submodule commit SHAs in `.gitmodules`. Only the constants
//! that the build code needs to know about at run time are kept
//! here.

/// MuPDF version expected in `thirdparty/mupdf/include/mupdf/fitz/version.h`.
///
/// Changing this constant will cause the native build to panic on
/// the first run after the change unless the submodule is also
/// updated. The CI cache key in
/// The shared cargo cache key in `.github/workflows/cargo.yml` includes
/// so caches stay consistent.
pub const MUPDF_VERSION: &str = "1.27.2";

/// All thirdparty libraries in dependency order for cross-compiling
/// to the Kobo target. Order matters: each library is built with the
/// previous one available on its include/library search path.
pub const LIBRARY_NAMES: &[&str] = &[
    "zlib",
    "bzip2",
    "libpng",
    "libjpeg",
    "openjpeg",
    "jbig2dec",
    "libwebp",
    "freetype2",
    "harfbuzz",
    "gumbo",
    "djvulibre",
    "mupdf",
];

/// Base names of the thirdparty shared libraries that must end up in
/// the Kobo `libs/` directory.
///
/// The base name (e.g. `libz.so`) is what the Cadmus runtime loads;
/// the actual SONAME suffix (`libz.so.1.2.13`) is discovered at
/// runtime via `arm-linux-gnueabihf-readelf -d` because upstream
/// libraries do not follow a consistent ABI versioning scheme.
pub const SONAMES: &[&str] = &[
    "libz.so",
    "libbz2.so",
    "libpng16.so",
    "libjpeg.so",
    "libopenjp2.so",
    "libjbig2dec.so",
    "libfreetype.so",
    "libharfbuzz.so",
    "libgumbo.so",
    "libwebp.so",
    "libwebpdemux.so",
    "libdjvulibre.so",
    "libmupdf.so",
];

/// Patch series applied to the MuPDF source tree to add WebP support.
///
/// Applied by [`crate::build::mupdf::apply_webp_patches_if_needed`]
/// and by [`crate::build::kobo::source::apply_patches`] when building MuPDF.
pub const MUPDF_WEBP_PATCHES: &[&str] = &[
    "webp-upstream-697749-kobo.patch",
    "webp-image-h-kobo.patch",
    "webp-load-webp-deviations-kobo.patch",
];

/// Cross-compilation environment variables injected when `cargo xtask
/// build-kobo` runs `cargo build` for the Kobo ARM target.
///
/// SQLite discovery uses `PKG_CONFIG_PATH_<target>` vars set in devenv and CI,
/// which are target-aware and route each build (host proc-macro + ARM) to its
/// own `libsqlite3.a` via pkg-config. No SQLite vars are needed here.
pub const CROSS_ENV: &[(&str, &str)] = &[
    ("PKG_CONFIG_ALLOW_CROSS", "1"),
    (
        "CARGO_TARGET_ARM_UNKNOWN_LINUX_GNUEABIHF_LINKER",
        "arm-linux-gnueabihf-gcc",
    ),
    ("CC_arm_unknown_linux_gnueabihf", "arm-linux-gnueabihf-gcc"),
    ("AR_arm_unknown_linux_gnueabihf", "arm-linux-gnueabihf-ar"),
];

/// Mapping from built `.so` paths to their destination names in the
/// Kobo `libs/` directory.
///
/// The source paths are relative to the workspace root and use the
/// `thirdparty/<lib>/...` form so that the same table can be
/// re-targeted at a per-target build root by stripping the
/// `thirdparty/` prefix (see [`crate::build::kobo::copy_built_libs`]).
pub const BUILT_LIBRARY_COPIES: &[(&str, &str)] = &[
    ("thirdparty/zlib/libz.so", "libz.so"),
    ("thirdparty/bzip2/libbz2.so", "libbz2.so"),
    ("thirdparty/libpng/.libs/libpng16.so", "libpng16.so"),
    ("thirdparty/libjpeg/.libs/libjpeg.so", "libjpeg.so"),
    (
        "thirdparty/openjpeg/build/bin/libopenjp2.so",
        "libopenjp2.so",
    ),
    ("thirdparty/jbig2dec/.libs/libjbig2dec.so", "libjbig2dec.so"),
    ("thirdparty/libwebp/src/.libs/libwebp.so", "libwebp.so"),
    (
        "thirdparty/libwebp/src/demux/.libs/libwebpdemux.so",
        "libwebpdemux.so",
    ),
    (
        "thirdparty/freetype2/objs/.libs/libfreetype.so",
        "libfreetype.so",
    ),
    (
        "thirdparty/harfbuzz/build/src/libharfbuzz.so",
        "libharfbuzz.so",
    ),
    ("thirdparty/gumbo/.libs/libgumbo.so", "libgumbo.so"),
    (
        "thirdparty/djvulibre/libdjvu/.libs/libdjvulibre.so",
        "libdjvulibre.so",
    ),
    ("thirdparty/mupdf/build/release/libmupdf.so", "libmupdf.so"),
];
