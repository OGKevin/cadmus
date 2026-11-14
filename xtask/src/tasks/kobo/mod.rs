//! `cadmuscli kobo` — deploy and restart Cadmus on a Kobo over SSH.
//!
//! `deploy` cross-compiles the test feature set, assembles `dist/`, and
//! uploads it over SFTP. `restart` sends `SIGUSR1` to the Cadmus process whose
//! working directory is [`INSTALL_DIR`]. Both share the SSH transport in
//! [`ssh`], which resolves `~/.ssh/config`, host keys, and authentication in
//! process so the plugin never shells out to `ssh` or `rsync`.

mod deploy;
mod restart;
mod ssh;
mod upload;

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};

/// Where a test deploy installs Cadmus on the device.
pub(crate) const INSTALL_DIR: &str = "/mnt/onboard/.adds/cadmus-tst";

/// Features every test deploy pins, in this order, ahead of any extras.
const REQUIRED_FEATURES: [&str; 2] = ["test", "kobo"];

/// Arguments for `cadmuscli kobo`.
#[derive(Debug, Args)]
pub struct KoboArgs {
    #[command(subcommand)]
    pub command: KoboCommand,
}

#[derive(Debug, Subcommand)]
pub enum KoboCommand {
    /// Build the test feature set, assemble `dist/`, and upload it over SFTP
    /// to `/mnt/onboard/.adds/cadmus-tst` (the only supported deployment).
    Deploy(DeployArgs),
    /// Restart the Cadmus process running from the test install.
    Restart(RestartArgs),
}

/// SSH transport options shared by `kobo deploy` and `kobo restart`.
#[derive(Debug, Args)]
pub struct SshOptions {
    /// IP address or `~/.ssh/config` alias of the Kobo (default: `KOBO_IP`).
    #[arg(
        long,
        env = "KOBO_IP",
        value_name = "HOST",
        value_parser = clap::builder::NonEmptyStringValueParser::new()
    )]
    pub ip: String,

    /// SSH port (default: ssh_config, else 22).
    #[arg(long)]
    pub port: Option<u16>,

    /// SSH user (default: ssh_config, else root).
    #[arg(long)]
    pub user: Option<String>,

    /// Private key file; repeatable. Tried before ssh_config IdentityFile and
    /// the usual `~/.ssh` keys.
    #[arg(long, value_name = "PATH")]
    pub identity: Vec<PathBuf>,

    /// Only use the configured identity files and matching ssh-agent keys;
    /// never offer unrelated agent identities (`IdentitiesOnly` in ssh_config
    /// is not read).
    #[arg(long)]
    pub identities_only: bool,

    /// known_hosts file (default: ssh_config UserKnownHostsFile, else
    /// `~/.ssh/known_hosts`).
    #[arg(long, value_name = "PATH")]
    pub known_hosts: Option<PathBuf>,

    /// Learn an unknown host key on first connect (trust on first use).
    #[arg(long)]
    pub accept_new: bool,

    /// ssh_config file (default `~/.ssh/config`).
    #[arg(long, value_name = "PATH")]
    pub ssh_config: Option<PathBuf>,
}

/// Arguments for `cadmuscli kobo deploy`.
#[derive(Debug, Args)]
pub struct DeployArgs {
    #[command(flatten)]
    pub ssh: SshOptions,

    /// Extra Cargo features to add on top of `test,kobo` (e.g. `tracing`).
    #[arg(long)]
    pub features: Option<String>,

    /// Restart Cadmus on the device once the upload finishes.
    #[arg(long)]
    pub restart: bool,
}

/// Arguments for `cadmuscli kobo restart`.
#[derive(Debug, Args)]
pub struct RestartArgs {
    #[command(flatten)]
    pub ssh: SshOptions,
}

/// Dispatches a `kobo` subcommand.
pub async fn run(args: KoboArgs) -> Result<()> {
    match args.command {
        KoboCommand::Deploy(args) => deploy::run(args).await,
        KoboCommand::Restart(args) => restart::run(args).await,
    }
}

/// Composes the deploy feature set: the pinned `test,kobo` pair plus validated
/// extras.
///
/// The order is presentation only — `build-kobo` re-sorts the set before
/// handing it to cargo. Extras are split on `,` and `+`, deduplicated, and
/// rejected when they are not valid Cargo feature names so a typo cannot
/// smuggle shell syntax into the build.
pub(crate) fn deploy_features(extra: Option<&str>) -> Result<String> {
    let mut features: Vec<&str> = REQUIRED_FEATURES.to_vec();

    if let Some(extra) = extra {
        for part in extra.split([',', '+']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let valid = part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
            if !valid {
                bail!("invalid feature name {part:?}: expected letters, digits, '_' or '-'");
            }
            if !features.contains(&part) {
                features.push(part);
            }
        }
    }

    Ok(features.join(","))
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use crate::{Cli, Command};

    use super::*;

    #[test]
    fn deploy_features_pins_test_and_kobo_first() {
        assert_eq!(deploy_features(None).unwrap(), "test,kobo");
        assert_eq!(
            deploy_features(Some("tracing")).unwrap(),
            "test,kobo,tracing"
        );
        assert_eq!(
            deploy_features(Some("kobo,tracing,test")).unwrap(),
            "test,kobo,tracing"
        );
        assert_eq!(
            deploy_features(Some("test+kobo+tracing")).unwrap(),
            "test,kobo,tracing"
        );
    }

    #[test]
    fn deploy_features_rejects_invalid_names() {
        assert!(deploy_features(Some("tracing; reboot")).is_err());
        assert!(deploy_features(Some("tracing --verbose")).is_err());
    }

    #[test]
    fn parses_deploy_with_every_option() {
        let cli = Cli::try_parse_from([
            "cadmuscli",
            "kobo",
            "deploy",
            "--ip",
            "192.168.1.128",
            "--features",
            "tracing",
            "--restart",
            "--port",
            "2222",
            "--user",
            "root",
            "--identity",
            "/tmp/key",
            "--known-hosts",
            "/tmp/known_hosts",
            "--accept-new",
            "--ssh-config",
            "/tmp/ssh_config",
        ])
        .unwrap();

        let Command::Kobo(KoboArgs {
            command: KoboCommand::Deploy(args),
        }) = cli.command
        else {
            panic!("expected `kobo deploy`");
        };

        assert_eq!(args.ssh.ip, "192.168.1.128");
        assert_eq!(args.features.as_deref(), Some("tracing"));
        assert!(args.restart);
        assert_eq!(args.ssh.port, Some(2222));
        assert_eq!(args.ssh.user.as_deref(), Some("root"));
        assert_eq!(args.ssh.identity, vec![PathBuf::from("/tmp/key")]);
        assert_eq!(
            args.ssh.known_hosts,
            Some(PathBuf::from("/tmp/known_hosts"))
        );
        assert!(args.ssh.accept_new);
        assert_eq!(args.ssh.ssh_config, Some(PathBuf::from("/tmp/ssh_config")));
    }

    #[test]
    fn parses_restart_with_defaults() {
        let cli =
            Cli::try_parse_from(["cadmuscli", "kobo", "restart", "--ip", "kobo.local"]).unwrap();

        let Command::Kobo(KoboArgs {
            command: KoboCommand::Restart(args),
        }) = cli.command
        else {
            panic!("expected `kobo restart`");
        };

        assert_eq!(args.ssh.ip, "kobo.local");
        assert_eq!(args.ssh.port, None);
        assert_eq!(args.ssh.user, None);
        assert!(args.ssh.identity.is_empty());
        assert_eq!(args.ssh.known_hosts, None);
        assert!(!args.ssh.accept_new);
        assert_eq!(args.ssh.ssh_config, None);
    }

    #[test]
    fn device_address_falls_back_to_kobo_ip() {
        let command = Cli::command();
        let kobo = command.find_subcommand("kobo").unwrap();

        for name in ["deploy", "restart"] {
            let subcommand = kobo.find_subcommand(name).unwrap();
            let ip = subcommand
                .get_arguments()
                .find(|arg| arg.get_id() == "ip")
                .unwrap();
            assert_eq!(
                ip.get_env(),
                Some(std::ffi::OsStr::new("KOBO_IP")),
                "{name} does not read KOBO_IP"
            );
        }
    }
}
