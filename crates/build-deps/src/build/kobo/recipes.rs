//! Per-library build recipes for the Kobo cross-build.
//!
//! Each recipe matches the upstream build system of one thirdparty
//! library (autotools, CMake, plain `cc` invocation, meson). All
//! recipes share the same CFLAGS via [`cross_env`], targeting the
//! Kobo's Cortex-A9 CPU with NEON.

use std::path::Path;

use anyhow::{Context, Result};

use crate::cmd;

/// Common cross-compilation environment used by most recipes.
fn cross_env() -> [(&'static str, &'static str); 12] {
    [
        ("CC", "arm-linux-gnueabihf-gcc"),
        ("CC_BUILD", "cc"),
        ("CXX", "arm-linux-gnueabihf-g++"),
        ("AR", "arm-linux-gnueabihf-ar"),
        ("AS", "arm-linux-gnueabihf-as"),
        ("NM", "arm-linux-gnueabihf-nm"),
        ("STRIP", "arm-linux-gnueabihf-strip"),
        ("RANLIB", "arm-linux-gnueabihf-ranlib"),
        ("LD", "arm-linux-gnueabihf-ld"),
        ("OBJDUMP", "arm-linux-gnueabihf-objdump"),
        ("CFLAGS", "-O2 -mcpu=cortex-a9 -mfpu=neon"),
        ("CXXFLAGS", "-O2 -mcpu=cortex-a9 -mfpu=neon"),
    ]
}

/// Build a single library by name, dispatching to the recipe that
/// matches its upstream build system.
pub fn build_library(name: &str, build_dir: &Path) -> Result<()> {
    println!("Building {name}...");

    let env = cross_env();
    match name {
        "zlib" => build_zlib(build_dir, &env),
        "bzip2" => build_bzip2(build_dir),
        "libpng" => build_libpng(build_dir),
        "libjpeg" => build_libjpeg(build_dir, &env),
        "openjpeg" => build_openjpeg(build_dir, &env),
        "jbig2dec" => build_jbig2dec(build_dir, &env),
        "libwebp" => build_libwebp(build_dir, &env),
        "freetype2" => build_freetype2(build_dir),
        "harfbuzz" => build_harfbuzz(build_dir),
        "gumbo" => build_gumbo(build_dir, &env),
        "djvulibre" => build_djvulibre(build_dir),
        "mupdf" => super::mupdf::build_mupdf(build_dir),
        _ => anyhow::bail!("unknown library: {name}"),
    }
}

fn build_zlib(build_dir: &Path, env: &[(&str, &str)]) -> Result<()> {
    let zlib_env = {
        let mut e: Vec<(&str, &str)> = env.to_vec();
        e.push(("CHOST", "arm-linux-gnueabihf"));
        e
    };
    cmd::run("./configure", &[], build_dir, &zlib_env).context("failed to configure zlib")?;
    cmd::run(
        "make",
        &[
            "-j4",
            "AR=arm-linux-gnueabihf-ar",
            "ARFLAGS=rc",
            "RANLIB=arm-linux-gnueabihf-ranlib",
        ],
        build_dir,
        env,
    )
    .context("failed to build zlib")
}

fn build_bzip2(build_dir: &Path) -> Result<()> {
    let cc = "arm-linux-gnueabihf-gcc";
    let cflags: Vec<&str> =
        "-fpic -fPIC -Wall -Winline -O2 -mcpu=cortex-a9 -mfpu=neon -g -D_FILE_OFFSET_BITS=64"
            .split_whitespace()
            .collect();

    let sources = [
        "blocksort.c",
        "huffman.c",
        "crctable.c",
        "randtable.c",
        "compress.c",
        "decompress.c",
        "bzlib.c",
    ];

    for src in &sources {
        let obj = src.replace(".c", ".o");
        let mut args: Vec<&str> = vec!["-c", src, "-o", &obj];
        args.extend(cflags.iter());
        cmd::run(cc, &args, build_dir, &[]).with_context(|| format!("failed to compile {src}"))?;
    }

    let objs: Vec<String> = sources.iter().map(|s| s.replace(".c", ".o")).collect();
    let obj_refs: Vec<&str> = objs.iter().map(|s| s.as_str()).collect();

    let mut link_args: Vec<&str> = vec![
        "-shared",
        "-Wl,-soname",
        "-Wl,libbz2.so.1.0",
        "-o",
        "libbz2.so.1.0.6",
    ];
    link_args.extend(obj_refs.iter());
    cmd::run(cc, &link_args, build_dir, &[]).context("failed to link libbz2.so")?;

    let so_1_0 = build_dir.join("libbz2.so.1.0");
    if so_1_0.exists() {
        std::fs::remove_file(&so_1_0)?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("libbz2.so.1.0.6", &so_1_0)?;

    let so_unversioned = build_dir.join("libbz2.so");
    if so_unversioned.exists() {
        std::fs::remove_file(&so_unversioned)?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("libbz2.so.1.0.6", &so_unversioned)?;

    Ok(())
}

fn build_libpng(build_dir: &Path) -> Result<()> {
    let env = [
        ("CC", "arm-linux-gnueabihf-gcc"),
        ("CC_BUILD", "cc"),
        ("CXX", "arm-linux-gnueabihf-g++"),
        ("AR", "arm-linux-gnueabihf-ar"),
        ("AS", "arm-linux-gnueabihf-as"),
        ("LD", "arm-linux-gnueabihf-ld"),
        ("NM", "arm-linux-gnueabihf-nm"),
        ("OBJDUMP", "arm-linux-gnueabihf-objdump"),
        ("RANLIB", "arm-linux-gnueabihf-ranlib"),
        ("STRIP", "arm-linux-gnueabihf-strip"),
        ("CFLAGS", "-O2 -mcpu=cortex-a9 -mfpu=neon"),
        ("CXXFLAGS", "-O2 -mcpu=cortex-a9 -mfpu=neon"),
        ("CPPFLAGS", "-I../zlib"),
        ("LDFLAGS", "-L../zlib"),
    ];
    cmd::run(
        "./configure",
        &["--host=arm-linux-gnueabihf"],
        build_dir,
        &env,
    )
    .context("failed to configure libpng")?;
    cmd::run("make", &["-j4"], build_dir, &env).context("failed to build libpng")
}

fn build_libjpeg(build_dir: &Path, env: &[(&str, &str)]) -> Result<()> {
    cmd::run(
        "./configure",
        &["--host=arm-linux-gnueabihf"],
        build_dir,
        env,
    )
    .context("failed to configure libjpeg")?;
    cmd::run("make", &["-j4"], build_dir, env).context("failed to build libjpeg")?;

    let include_link = build_dir.join("include");
    if !include_link.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(".", &include_link)?;
    }
    let lib_link = build_dir.join("lib");
    if !lib_link.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(".libs", &lib_link)?;
    }
    Ok(())
}

fn build_openjpeg(build_dir: &Path, env: &[(&str, &str)]) -> Result<()> {
    let cmake_build = build_dir.join("build");
    if cmake_build.exists() {
        std::fs::remove_dir_all(&cmake_build)?;
    }
    std::fs::create_dir(&cmake_build)?;

    cmd::run(
        "cmake",
        &[
            "-DCMAKE_BUILD_TYPE=Release",
            "-DBUILD_CODEC=off",
            "-DBUILD_STATIC_LIBS=off",
            "-DCMAKE_SYSTEM_NAME=Linux",
            "-DCMAKE_C_COMPILER=arm-linux-gnueabihf-gcc",
            "-DCMAKE_CXX_COMPILER=arm-linux-gnueabihf-g++",
            "-DCMAKE_AR=arm-linux-gnueabihf-ar",
            "..",
        ],
        &cmake_build,
        env,
    )
    .context("failed to configure openjpeg")?;
    cmd::run("make", &["-j4"], &cmake_build, env).context("failed to build openjpeg")?;

    let config_src = cmake_build.join("src/lib/openjp2/opj_config.h");
    let config_dest = build_dir.join("src/lib/openjp2/opj_config.h");
    if config_src.exists() {
        std::fs::copy(&config_src, &config_dest)?;
    }
    Ok(())
}

fn build_jbig2dec(build_dir: &Path, env: &[(&str, &str)]) -> Result<()> {
    cmd::run(
        "./autogen.sh",
        &["--host=arm-linux-gnueabihf"],
        build_dir,
        env,
    )
    .context("failed to run autogen.sh for jbig2dec")?;
    cmd::run("make", &["-j4"], build_dir, env).context("failed to build jbig2dec")
}

fn build_libwebp(build_dir: &Path, env: &[(&str, &str)]) -> Result<()> {
    cmd::run("make", &["distclean"], build_dir, &[]).ok();
    if !build_dir.join("configure").exists() {
        cmd::run("sh", &["autogen.sh"], build_dir, &[("NOCONFIGURE", "1")])
            .context("failed to run autogen.sh for libwebp")?;
    }
    cmd::run(
        "./configure",
        &[
            "--host=arm-linux-gnueabihf",
            "--enable-shared",
            "--disable-static",
            "--disable-libwebpmux",
            "--enable-libwebpdecoder",
            "--enable-libwebpdemux",
            "--disable-webp-tools",
        ],
        build_dir,
        env,
    )
    .context("failed to configure libwebp")?;
    cmd::run("make", &["-j4"], build_dir, env).context("failed to build libwebp")
}

fn build_freetype2(build_dir: &Path) -> Result<()> {
    let env = [
        ("CC", "arm-linux-gnueabihf-gcc"),
        ("CC_BUILD", "cc"),
        ("CXX", "arm-linux-gnueabihf-g++"),
        ("AR", "arm-linux-gnueabihf-ar"),
        ("AS", "arm-linux-gnueabihf-as"),
        ("LD", "arm-linux-gnueabihf-ld"),
        ("NM", "arm-linux-gnueabihf-nm"),
        ("OBJDUMP", "arm-linux-gnueabihf-objdump"),
        ("RANLIB", "arm-linux-gnueabihf-ranlib"),
        ("STRIP", "arm-linux-gnueabihf-strip"),
        ("CFLAGS", "-O2 -mcpu=cortex-a9 -mfpu=neon"),
        ("CXXFLAGS", "-O2 -mcpu=cortex-a9 -mfpu=neon"),
        ("ZLIB_CFLAGS", "-I../zlib"),
        ("ZLIB_LIBS", "-L../zlib -lz"),
        ("BZIP2_CFLAGS", "-I../bzip2"),
        ("BZIP2_LIBS", "-L../bzip2 -lbz2"),
        ("LIBPNG_CFLAGS", "-I../libpng"),
        ("LIBPNG_LIBS", "-L../libpng/.libs -lpng16"),
    ];

    std::fs::create_dir_all(build_dir.join("objs"))?;

    let autogen = build_dir.join("autogen.sh");
    if autogen.exists() {
        cmd::run("./autogen.sh", &[], build_dir, &env)
            .context("failed to run autogen.sh for freetype2")?;
    }

    cmd::run(
        "./configure",
        &[
            "--host=arm-linux-gnueabihf",
            "--with-zlib=yes",
            "--with-png=yes",
            "--with-bzip2=yes",
            "--with-harfbuzz=no",
            "--with-brotli=no",
            "--disable-static",
        ],
        build_dir,
        &env,
    )
    .context("failed to configure freetype2")?;
    cmd::run("make", &["-j4"], build_dir, &env).context("failed to build freetype2")
}

fn build_harfbuzz(build_dir: &Path) -> Result<()> {
    cmd::run(
        "meson",
        &[
            "setup",
            "-Dglib=disabled",
            "-Dicu=disabled",
            "-Dcairo=disabled",
            "-Dfreetype=enabled",
            "--cross-file",
            "kobo-options.txt",
            "build",
        ],
        build_dir,
        &[],
    )
    .context("failed to configure harfbuzz")?;
    cmd::run("meson", &["compile", "-C", "build"], build_dir, &[])
        .context("failed to build harfbuzz")
}

fn build_gumbo(build_dir: &Path, env: &[(&str, &str)]) -> Result<()> {
    if !build_dir.join("configure").exists() {
        cmd::run("./autogen.sh", &[], build_dir, env)
            .context("failed to run autogen.sh for gumbo")?;
    }
    cmd::run(
        "./configure",
        &["--host=arm-linux-gnueabihf"],
        build_dir,
        env,
    )
    .context("failed to configure gumbo")?;
    cmd::run("make", &["-j4"], build_dir, env).context("failed to build gumbo")
}

fn build_djvulibre(build_dir: &Path) -> Result<()> {
    let parent = build_dir.parent().context("build_dir has no parent")?;
    let jpeg_abs =
        std::fs::canonicalize(parent.join("libjpeg")).context("failed to resolve libjpeg path")?;
    let jpeg_str = jpeg_abs
        .to_str()
        .context("jpeg path not UTF-8")?
        .to_string();
    let jpeg_cflags = format!("-I{}", jpeg_str);
    let jpeg_libs = format!("-L{}/.libs -ljpeg", jpeg_str);
    let jpeg_with_arg = format!("--with-jpeg={}", jpeg_str);

    let env = [
        ("CC", "arm-linux-gnueabihf-gcc"),
        ("CXX", "arm-linux-gnueabihf-g++"),
        ("AR", "arm-linux-gnueabihf-ar"),
        ("AS", "arm-linux-gnueabihf-as"),
        ("LD", "arm-linux-gnueabihf-ld"),
        ("NM", "arm-linux-gnueabihf-nm"),
        ("OBJDUMP", "arm-linux-gnueabihf-objdump"),
        ("RANLIB", "arm-linux-gnueabihf-ranlib"),
        ("STRIP", "arm-linux-gnueabihf-strip"),
        ("CFLAGS", "-O2 -mcpu=cortex-a9 -mfpu=neon"),
        ("CXXFLAGS", "-O2 -mcpu=cortex-a9 -mfpu=neon"),
        ("JPEG_DIR", &jpeg_str),
        ("JPEG_CFLAGS", &jpeg_cflags),
        ("JPEG_LIBS", &jpeg_libs),
    ];
    cmd::run(
        "./autogen.sh",
        &[
            "--host=arm-linux-gnueabihf",
            "--disable-xmltools",
            "--disable-desktopfiles",
            &jpeg_with_arg,
        ],
        build_dir,
        &env,
    )
    .context("failed to run autogen.sh for djvulibre")?;
    cmd::run("make", &["-j4"], build_dir, &env).context("failed to build djvulibre")
}
