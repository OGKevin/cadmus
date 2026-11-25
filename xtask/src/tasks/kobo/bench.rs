//! `cadmuscli kobo bench` — run one criterion benchmark on a Kobo.
//!
//! The benchmark is cross-compiled for ARM, pushed with the in-process rsync
//! engine into [`BENCH_DIR`]`/<name>/` on the device's flash storage, run over
//! SSH with its output streamed live, and its criterion output is copied back to
//! `target/kobo-bench/<name>/`. The installed Cadmus under
//! [`INSTALL_DIR`](super::INSTALL_DIR) is never touched, and the scratch
//! directory handed to the benchmark lives on flash instead of the RAM-backed
//! `/tmp`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use build_deps::build::sqlite;
use build_deps::versions::CROSS_ENV;
use serde::Deserialize;

use crate::blocking;
use crate::tasks::build_kobo;
use crate::tasks::dist;
use crate::tasks::util::{cmd, sqlite_preflight, workspace};

use super::{BENCH_DIR, BenchArgs, rsync, ssh};

const TARGET: &str = "arm-unknown-linux-gnueabihf";

/// The `bench` feature swaps the device for `TestDevice`, which collides with
/// the `kobo` feature's device, so a device-less build carries the benchmarks.
const BENCH_FEATURES: &str = "deviceless,bench";

/// Compiler flag override for the cross build.
///
/// The devenv sets `CC=clang`, which makes the `alloca` build script (a
/// criterion dependency) add `-flto`. The Linaro gcc then emits slim LTO
/// objects its own linker cannot resolve, so LTO is switched off for C code.
const CROSS_CFLAGS: (&str, &str) = ("CFLAGS_arm_unknown_linux_gnueabihf", "-fno-lto");

/// File name of the benchmark binary inside the staging directory, as
/// [`BENCH_SCRIPT`] runs it.
const BINARY: &str = "bench";

/// Remote script that runs the staged benchmark; a standalone file so
/// shellcheck and shfmt lint it like every other shell script.
const BENCH_SCRIPT: &str = include_str!("bench.sh");

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    target_directory: PathBuf,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    targets: Vec<Target>,
}

#[derive(Debug, Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
}

/// A criterion benchmark target of a workspace package.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BenchTarget {
    package: String,
    name: String,
}

/// One line of cargo's `--message-format=json` output.
#[derive(Debug, Deserialize)]
struct Message {
    reason: String,
    target: Option<Target>,
    executable: Option<PathBuf>,
}

/// Builds, pushes, runs, and collects one benchmark.
pub(crate) async fn run(args: BenchArgs) -> Result<()> {
    let root = blocking(workspace::root).await?;
    let metadata = cargo_metadata(&root).await?;
    let target = select(&bench_targets(&metadata), &args.bench)?.clone();
    let settings = ssh::resolve(&args.ssh).await?;

    println!("Building {} for ARM...", target.name);
    let stage = {
        let root = root.clone();
        let target = target.clone();
        let target_dir = metadata.target_directory.clone();
        blocking(move || build_stage(&root, &target, &target_dir)).await?
    };

    let remote = remote_dir(&target.name);
    let mut handle = ssh::connect(&settings).await?;

    let prepared = ssh::exec(&mut handle, &format!("mkdir -p {}", shell_quote(&remote))).await?;
    if prepared.exit_status != Some(0) {
        bail!(
            "failed to create {remote} on {}: {}",
            settings.host(),
            prepared.stderr.trim()
        );
    }

    println!(
        "Syncing {} to {} ({remote})...",
        target.name,
        settings.host()
    );
    rsync::run(&mut handle, &stage, &remote).await?;

    println!("Running {} on {}...", target.name, settings.host());
    let status = ssh::exec_to(
        &mut handle,
        &run_script(&remote, &with_noplot(&args.args)),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
    .await?;
    if status != Some(0) {
        bail!(
            "benchmark {} exited with status {} on {}",
            target.name,
            status.map_or_else(|| "unknown".to_owned(), |code| code.to_string()),
            settings.host()
        );
    }

    let results = metadata
        .target_directory
        .join("kobo-bench")
        .join(&target.name);
    if pull_results(&mut handle, &remote, &results).await? {
        println!("Results copied to {}", results.display());
        println!("{}", view_hint(&results, &target, &args.args));
    } else {
        println!("The benchmark wrote no criterion output; nothing to copy.");
    }
    Ok(())
}

/// Runs `cargo metadata` for the workspace.
async fn cargo_metadata(root: &Path) -> Result<Metadata> {
    let root = root.to_path_buf();
    let json = blocking(move || {
        cmd::output(
            "cargo",
            &["metadata", "--format-version=1", "--no-deps"],
            &root,
            &[],
        )
    })
    .await?;
    serde_json::from_str(&json).context("failed to parse cargo metadata JSON")
}

/// Lists every bench target of the workspace members, sorted by name.
fn bench_targets(metadata: &Metadata) -> Vec<BenchTarget> {
    let mut targets: Vec<BenchTarget> = metadata
        .packages
        .iter()
        .flat_map(|package| {
            package
                .targets
                .iter()
                .filter(|target| target.kind.iter().any(|kind| kind == "bench"))
                .map(|target| BenchTarget {
                    package: package.name.clone(),
                    name: target.name.clone(),
                })
        })
        .collect();
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    targets
}

/// Finds `name` among `targets`, or fails listing the available names.
fn select<'a>(targets: &'a [BenchTarget], name: &str) -> Result<&'a BenchTarget> {
    targets
        .iter()
        .find(|target| target.name == name)
        .ok_or_else(|| {
            let names: Vec<&str> = targets.iter().map(|target| target.name.as_str()).collect();
            anyhow!(
                "unknown benchmark {name:?}; available benchmarks: {}",
                names.join(", ")
            )
        })
}

/// Arguments for the cross build of one benchmark.
fn cargo_args(target: &BenchTarget) -> Vec<String> {
    [
        "bench",
        "--no-run",
        "--target",
        TARGET,
        "-p",
        &target.package,
        "--features",
        BENCH_FEATURES,
        "--bench",
        &target.name,
        "--message-format=json-render-diagnostics",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Reads the benchmark executable path from cargo's JSON messages.
fn executable(cargo_stdout: &str, name: &str) -> Result<PathBuf> {
    cargo_stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Message>(line).ok())
        .filter(|message| message.reason == "compiler-artifact")
        .filter(|message| {
            message.target.as_ref().is_some_and(|target| {
                target.name == name && target.kind.iter().any(|kind| kind == "bench")
            })
        })
        .filter_map(|message| message.executable)
        .next_back()
        .ok_or_else(|| anyhow!("cargo did not report an executable for benchmark {name}"))
}

/// Cross-builds the benchmark and assembles the directory that gets pushed:
/// the stripped binary plus the versioned shared libraries it links.
fn build_stage(root: &Path, target: &BenchTarget, target_dir: &Path) -> Result<PathBuf> {
    build_kobo::ensure_linaro_toolchain()?;
    sqlite_preflight::ensure_for_target(root, sqlite::KOBO_TARGET)
        .context("failed to build SQLite for Kobo")?;

    let args = cargo_args(target);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut env = CROSS_ENV.to_vec();
    env.push(CROSS_CFLAGS);
    let stdout = cmd::output("cargo", &args, root, &env)?;
    let built = executable(&stdout, &target.name)?;

    let stage = target_dir.join("kobo-bench-stage").join(&target.name);
    if stage.exists() {
        std::fs::remove_dir_all(&stage)
            .with_context(|| format!("failed to clear {}", stage.display()))?;
    }
    std::fs::create_dir_all(stage.join("libs"))
        .with_context(|| format!("failed to create {}", stage.display()))?;

    dist::copy_libraries(root, &stage)?;
    let binary = stage.join(BINARY);
    std::fs::copy(&built, &binary)
        .with_context(|| format!("failed to copy {}", built.display()))?;
    dist::strip_and_patch_files(root, &binary, &stage.join("libs"))?;
    Ok(stage)
}

/// Staging directory of one benchmark on the device.
fn remote_dir(name: &str) -> String {
    format!("{BENCH_DIR}/{name}")
}

/// The benchmark's arguments with `--noplot` added unless already present.
///
/// Rendering plots is slow on a Kobo, so it is off by default and the report is
/// rendered on the host afterwards (see [`view_hint`]). Criterion's `noplot`
/// flag takes no value, so a repeat would be rejected; `-n` is its short form.
fn with_noplot(bench_args: &[String]) -> Vec<String> {
    let mut args = bench_args.to_vec();
    if !args.iter().any(|arg| arg == "--noplot" || arg == "-n") {
        args.insert(0, "--noplot".to_owned());
    }
    args
}

/// How to render the HTML report on the host from the copied results.
///
/// `--load-baseline` makes criterion load the stored `new` sample instead of
/// sampling again and write its analysis and report, so the host only builds
/// and starts the benchmark; the numbers stay the Kobo's. Criterion panics for
/// a benchmark that has no stored sample, so the arguments of the device run
/// (filters) are repeated to select exactly the benchmarks that were measured.
fn view_hint(results: &Path, target: &BenchTarget, bench_args: &[String]) -> String {
    let criterion = results.join("criterion");
    let mut command = format!(
        "CRITERION_HOME={home} cargo bench -p {package} --features {BENCH_FEATURES} \
         --bench {name} -- --load-baseline new --baseline base",
        home = shell_quote(&criterion.to_string_lossy()),
        package = target.package,
        name = target.name,
    );
    for arg in bench_args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    format!(
        "Plots are off on the device. To render the HTML report here from the copied data, run:\n  \
         {command}\n\
         then open {report}",
        report = criterion.join("report/index.html").display(),
    )
}

/// Quotes `value` as one POSIX shell word.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Remote script that runs the staged benchmark.
///
/// [`BENCH_SCRIPT`] reads the staging directory from the environment and the
/// benchmark's own arguments from the positional parameters, so both are
/// prepended here shell-quoted rather than substituted into the script text.
fn run_script(remote: &str, bench_args: &[String]) -> String {
    let mut script = format!("export CADMUS_BENCH_STAGE={}\nset --", shell_quote(remote));
    for arg in bench_args {
        script.push(' ');
        script.push_str(&shell_quote(arg));
    }
    script.push('\n');
    script.push_str(BENCH_SCRIPT);
    script
}

/// Copies `criterion/` from the device into `destination`, replacing it.
///
/// Returns `false` when the run wrote no criterion output (for example
/// `--list`), leaving `destination` untouched.
async fn pull_results(
    handle: &mut russh::client::Handle<ssh::Handler>,
    remote: &str,
    destination: &Path,
) -> Result<bool> {
    let present = ssh::exec(
        handle,
        &format!("test -d {}/criterion", shell_quote(remote)),
    )
    .await?;
    if present.exit_status != Some(0) {
        return Ok(false);
    }

    let mut archive = Vec::new();
    let mut stderr = Vec::new();
    let status = ssh::exec_to(
        handle,
        &format!("tar -C {} -cf - criterion", shell_quote(remote)),
        &mut archive,
        &mut stderr,
    )
    .await?;
    if status != Some(0) {
        bail!(
            "failed to read the criterion results on the device: {}",
            String::from_utf8_lossy(&stderr).trim()
        );
    }

    let destination = destination.to_path_buf();
    blocking(move || unpack(&archive, &destination)).await?;
    Ok(true)
}

/// Replaces `destination` with the contents of the tar `archive`.
fn unpack(archive: &[u8], destination: &Path) -> Result<()> {
    if destination.exists() {
        std::fs::remove_dir_all(destination)
            .with_context(|| format!("failed to clear {}", destination.display()))?;
    }
    std::fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    tar::Archive::new(archive)
        .unpack(destination)
        .with_context(|| format!("failed to unpack results into {}", destination.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(package: &str, name: &str) -> BenchTarget {
        BenchTarget {
            package: package.to_owned(),
            name: name.to_owned(),
        }
    }

    fn metadata() -> Metadata {
        serde_json::from_str(
            r#"{
                "target_directory": "/work/target",
                "packages": [
                    {"name": "cadmus-core", "targets": [
                        {"name": "dictionary_bench", "kind": ["bench"]},
                        {"name": "cadmus_core", "kind": ["lib"]},
                        {"name": "parse", "kind": ["bench"]}
                    ]},
                    {"name": "xtask", "targets": [{"name": "cadmuscli", "kind": ["bin"]}]}
                ]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn bench_targets_lists_only_benches_sorted() {
        assert_eq!(
            bench_targets(&metadata()),
            [
                target("cadmus-core", "dictionary_bench"),
                target("cadmus-core", "parse")
            ]
        );
    }

    #[test]
    fn select_picks_the_named_bench() {
        let targets = bench_targets(&metadata());
        assert_eq!(
            select(&targets, "parse").unwrap(),
            &target("cadmus-core", "parse")
        );
    }

    #[test]
    fn select_unknown_name_lists_the_available_ones() {
        let targets = bench_targets(&metadata());
        let error = select(&targets, "cadmuscli").unwrap_err().to_string();
        assert!(error.contains("\"cadmuscli\""), "{error}");
        assert!(error.ends_with("dictionary_bench, parse"), "{error}");
    }

    #[test]
    fn cargo_args_build_one_bench_for_arm_with_json_messages() {
        let args = cargo_args(&target("cadmus-core", "parse"));
        assert_eq!(
            args,
            [
                "bench",
                "--no-run",
                "--target",
                "arm-unknown-linux-gnueabihf",
                "-p",
                "cadmus-core",
                "--features",
                "deviceless,bench",
                "--bench",
                "parse",
                "--message-format=json-render-diagnostics",
            ]
        );
    }

    #[test]
    fn executable_comes_from_the_matching_bench_artifact() {
        let stdout = concat!(
            r#"{"reason":"compiler-artifact","target":{"name":"cadmus_core","kind":["lib"]},"executable":null}"#,
            "\n",
            r#"{"reason":"build-script-executed","package_id":"x"}"#,
            "\n",
            r#"{"reason":"compiler-artifact","target":{"name":"parse","kind":["bench"]},"executable":"/t/release/build/cadmus-core/abc/out/parse-abc"}"#,
            "\n",
            r#"{"reason":"build-finished","success":true}"#,
            "\n",
        );
        assert_eq!(
            executable(stdout, "parse").unwrap(),
            PathBuf::from("/t/release/build/cadmus-core/abc/out/parse-abc")
        );
        assert!(executable(stdout, "other").is_err());
    }

    #[test]
    fn shell_quote_survives_single_quotes() {
        assert_eq!(shell_quote("epub"), "'epub'");
        assert_eq!(shell_quote("it's $HOME"), r"'it'\''s $HOME'");
    }

    #[test]
    fn with_noplot_prepends_the_flag_once() {
        let filter = ["epub".to_owned()];
        assert_eq!(with_noplot(&filter), ["--noplot", "epub"]);

        let explicit = ["epub".to_owned(), "--noplot".to_owned()];
        assert_eq!(with_noplot(&explicit), ["epub", "--noplot"]);
        assert_eq!(with_noplot(&["-n".to_owned()]), ["-n"]);
        assert_eq!(with_noplot(&[]), ["--noplot"]);
    }

    #[test]
    fn view_hint_renders_only_what_the_run_measured() {
        let hint = view_hint(
            Path::new("/w/it's/target/kobo-bench/parse"),
            &target("cadmus-core", "parse"),
            &["normalize/sorted 10k".to_owned()],
        );
        assert!(hint.contains(
            "CRITERION_HOME='/w/it'\\''s/target/kobo-bench/parse/criterion' cargo bench \
             -p cadmus-core --features deviceless,bench --bench parse -- \
             --load-baseline new --baseline base 'normalize/sorted 10k'\n"
        ));
        assert!(hint.contains("/w/it's/target/kobo-bench/parse/criterion/report/index.html"));

        let unfiltered = view_hint(Path::new("/w"), &target("cadmus-core", "parse"), &[]);
        assert!(unfiltered.contains("--baseline base\n"));
    }

    #[test]
    fn run_script_hands_quoted_arguments_to_the_staged_binary() {
        let args = ["a b; reboot".to_owned(), "--noplot".to_owned()];
        let script = run_script("/mnt/onboard/.adds/cadmus-bench/parse", &args);
        assert!(
            script.starts_with(
                "export CADMUS_BENCH_STAGE='/mnt/onboard/.adds/cadmus-bench/parse'\n\
                 set -- 'a b; reboot' '--noplot'\n"
            ),
            "{script}"
        );
    }

    #[test]
    fn run_script_runs_the_binary_with_flash_scratch_and_cleans_up() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path();
        let binary = stage.join("bench");
        std::fs::write(
            &binary,
            "#!/bin/sh\n\
             printf '%s\\n' \"$CADMUS_BENCH_DIR\" \"$LD_LIBRARY_PATH\" \"$CRITERION_HOME\" \"$@\" \
             > \"$CRITERION_HOME.out\"\n\
             test -d \"$CADMUS_BENCH_DIR\" || exit 9\n\
             exit 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

        let args = ["a b; reboot".to_owned(), "--noplot".to_owned()];
        let script = run_script(stage.to_str().unwrap(), &args);
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .status()
            .unwrap();

        assert_eq!(status.code(), Some(3));
        let seen = std::fs::read_to_string(stage.join("criterion.out")).unwrap();
        let stage = stage.display();
        assert_eq!(
            seen,
            format!(
                "{stage}/scratch\n{stage}/libs\n{stage}/criterion\n--bench\na b; reboot\n--noplot\n"
            )
        );
        assert!(!dir.path().join("scratch").exists());
    }

    #[test]
    fn unpack_replaces_previous_results() {
        let mut builder = tar::Builder::new(Vec::new());
        let body = b"report";
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "criterion/report/index.html", &body[..])
            .unwrap();
        let archive = builder.into_inner().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("parse");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("stale.txt"), "old").unwrap();

        unpack(&archive, &destination).unwrap();

        assert_eq!(
            std::fs::read_to_string(destination.join("criterion/report/index.html")).unwrap(),
            "report"
        );
        assert!(!destination.join("stale.txt").exists());
    }
}
