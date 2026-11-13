//! SSH transport for `cadmuscli kobo`: settings resolution, connection,
//! host-key policy, authentication, and command execution.
//!
//! Settings come from three layers: CLI flags override the matching
//! `~/.ssh/config` entry, which overrides built-in defaults. Identity files
//! try `--identity` first, then the host-specific `IdentityFile` list, then
//! the default key names. Authentication tries those key files, the `none`
//! method, a password from `KOBO_PASSWORD`, and only then ssh-agent
//! identities, so a large agent cannot burn the server's auth attempts before
//! a working method is reached. `--identities-only` drops unrelated agent keys
//! entirely.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use russh::client::{self, Handle};
use russh::keys::agent::client::{AgentClient, AgentStream};

use super::SshOptions;

/// Fallback SSH user when neither the CLI nor ssh_config names one.
const DEFAULT_USER: &str = "root";

/// Key names tried after `--identity` and ssh_config's `IdentityFile` entries.
const DEFAULT_IDENTITY_NAMES: [&str; 3] = ["id_ed25519", "id_ecdsa", "id_rsa"];

/// How long the TCP/ProxyCommand connection and SSH handshake may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Keepalive cadence so a sleeping or unreachable device is detected
/// mid-session instead of leaving it hanging.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// Fully resolved SSH settings for one device.
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    /// Parsed ssh_config entry, kept so [`russh_config::Config::stream`] can
    /// honour `ProxyCommand` and expand its `%h`/`%p`/`%u` tokens.
    config: russh_config::Config,
    pub user: String,
    /// Key files tried in order when the agent does not succeed.
    pub identities: Vec<PathBuf>,
    pub known_hosts: PathBuf,
    /// Learn unknown host keys instead of rejecting them.
    pub accept_new: bool,
    /// Never offer unrelated ssh-agent identities.
    pub identities_only: bool,
}

impl Settings {
    /// Host actually connected to (ssh_config `Hostname`, else the CLI value).
    pub fn host(&self) -> &str {
        self.config.host()
    }

    pub fn port(&self) -> u16 {
        self.config.port()
    }
}

/// Resolves [`Settings`] from CLI options, ssh_config, and defaults.
///
/// A missing ssh_config file is not an error: the CLI host is used with
/// defaults, which keeps `--ip` usable on machines without an ssh config.
pub(crate) async fn resolve(opts: &SshOptions) -> Result<Settings> {
    let config_path = match &opts.ssh_config {
        Some(path) => path.clone(),
        None => home_dir()?.join(".ssh").join("config"),
    };

    let mut config = match parse_ssh_config(&config_path, &opts.ip).await? {
        Some(config) => config,
        None => russh_config::Config::default(&opts.ip),
    };

    let jump = config
        .host_config
        .proxy_jump
        .clone()
        .filter(|jump| !jump.eq_ignore_ascii_case("none"));
    if let Some(jump) = jump {
        bail!(
            "ProxyJump {jump} is not supported: russh-config parses it but ignores it when \
             opening the stream, so the connection would bypass the jump host. \
             Express the hop as ProxyCommand in {}",
            config_path.display()
        );
    }

    let host_config = &mut config.host_config;
    if let Some(port) = opts.port {
        host_config.port = Some(port);
    }
    if let Some(user) = opts.user.clone() {
        host_config.user = Some(user);
    }
    if let Some(known_hosts) = opts.known_hosts.clone() {
        host_config.user_known_hosts_file = Some(known_hosts);
    }
    if host_config
        .proxy_command
        .as_deref()
        .is_some_and(|command| command.eq_ignore_ascii_case("none"))
    {
        host_config.proxy_command = None;
    }

    let user = host_config
        .user
        .clone()
        .unwrap_or_else(|| DEFAULT_USER.to_string());
    let known_hosts = match host_config.user_known_hosts_file.clone() {
        Some(path) => path,
        None => home_dir()?.join(".ssh").join("known_hosts"),
    };
    let accept_new = opts.accept_new || host_config.strict_host_key_checking == Some(false);

    let mut candidates: Vec<PathBuf> = opts.identity.clone();
    candidates.extend(host_config.identity_file.clone().unwrap_or_default());
    let ssh_dir = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".ssh"));
    candidates.extend(
        DEFAULT_IDENTITY_NAMES
            .iter()
            .filter_map(|name| ssh_dir.as_ref().map(|dir| dir.join(name))),
    );

    let mut identities = Vec::with_capacity(candidates.len());
    for path in candidates {
        if !identities.contains(&path) {
            identities.push(path);
        }
    }

    config.user = Some(user.clone());
    Ok(Settings {
        config,
        user,
        identities,
        known_hosts,
        accept_new,
        identities_only: opts.identities_only,
    })
}

/// Parses an ssh_config file, returning `None` when it does not exist.
async fn parse_ssh_config(path: &Path, host: &str) -> Result<Option<russh_config::Config>> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };

    let normalized = normalize_ssh_config(&text)?;
    let config = russh_config::parse(&normalized, host)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(config))
}

/// Rewrites ssh_config text into the shape [`russh_config`] understands.
///
/// - Directives before the first `Host` line are legal global options, but
///   russh-config's parser rejects them with `HostNotFound`. Prefixing
///   `Host *` makes them the wildcard entry; earlier entries win in
///   russh-config, which matches OpenSSH's first-obtained-value rule.
/// - OpenSSH accepts whitespace or optional whitespace and one `=` between a
///   keyword and its argument; russh-config only splits on a space. Every
///   directive is rewritten to `Keyword value`, which also keeps
///   `ProxyJump=bastion` visible to the refusal check instead of dropped.
/// - `StrictHostKeyChecking accept-new` collapses to `yes` in russh-config.
///   Rewriting it to `no` keeps trust-on-first-use, and a changed key is
///   still rejected.
/// - `Match` blocks would be attributed to the preceding `Host` entry, so
///   they are refused instead of silently misapplied.
fn normalize_ssh_config(text: &str) -> Result<String> {
    let mut normalized = String::with_capacity(text.len() + "Host *\n".len());
    normalized.push_str("Host *\n");

    for line in text.lines() {
        let Some((keyword, value)) = directive(line)? else {
            normalized.push_str(line.trim_end());
            normalized.push('\n');
            continue;
        };

        if keyword.eq_ignore_ascii_case("match") {
            bail!(
                "ssh_config Match blocks are not supported; \
                 move those settings into a Host block or pass --ssh-config with a filtered file"
            );
        }

        let indent = &line[..line.len() - line.trim_start().len()];
        normalized.push_str(indent);
        if keyword.eq_ignore_ascii_case("stricthostkeychecking")
            && value.eq_ignore_ascii_case("accept-new")
        {
            normalized.push_str("StrictHostKeyChecking no\n");
        } else {
            normalized.push_str(keyword);
            normalized.push(' ');
            normalized.push_str(value);
            normalized.push('\n');
        }
    }

    Ok(normalized)
}

/// Splits an ssh_config line into its keyword and value.
///
/// OpenSSH accepts whitespace or optional whitespace and one `=` between the
/// two. Directives without an argument are rejected instead of dropped, so a
/// malformed `ProxyJump` cannot slip through.
fn directive(line: &str) -> Result<Option<(&str, &str)>> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }

    let keyword_end = trimmed
        .find(|character: char| character.is_whitespace() || character == '=')
        .unwrap_or(trimmed.len());
    let keyword = &trimmed[..keyword_end];
    if keyword.is_empty() {
        bail!("malformed ssh_config directive: {trimmed:?}");
    }

    let rest = trimmed[keyword_end..].trim_start();
    let rest = rest
        .strip_prefix('=')
        .map_or(rest, |after| after.trim_start());
    let value = rest.trim_end();
    if value.is_empty() {
        bail!("ssh_config directive {keyword:?} has no argument");
    }

    Ok(Some((keyword, value)))
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set; pass --ssh-config, --identity and --known-hosts explicitly")
}

/// How to treat the server key reported by `known_hosts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostKeyDecision {
    /// The recorded key matches.
    Accept,
    /// Unknown host and the policy allows learning the key.
    Learn,
    /// Unknown host and the policy is strict.
    RejectUnknown,
    /// The recorded key differs; never learn over a mismatch.
    RejectChanged,
}

/// What the known_hosts file says about the server key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KnownHostStatus {
    /// The recorded key matches.
    Matched,
    /// No key of any algorithm is recorded for the host and port.
    Unknown,
    /// A different key of the same algorithm is recorded.
    KeyChanged,
    /// The host is recorded only under a different algorithm.
    AlgorithmMismatch,
}

/// Decides what to do with a [`KnownHostStatus`].
pub(crate) fn decide_host_key(status: KnownHostStatus, accept_new: bool) -> HostKeyDecision {
    match status {
        KnownHostStatus::Matched => HostKeyDecision::Accept,
        KnownHostStatus::Unknown if accept_new => HostKeyDecision::Learn,
        KnownHostStatus::Unknown => HostKeyDecision::RejectUnknown,
        KnownHostStatus::KeyChanged | KnownHostStatus::AlgorithmMismatch => {
            HostKeyDecision::RejectChanged
        }
    }
}

/// Reads the known_hosts entries for `host:port`.
///
/// Only a missing file counts as an empty key list; any other read or parse
/// error is propagated so an unreadable known_hosts fails closed. A host that
/// is recorded under a different algorithm is reported as a mismatch instead
/// of being treated as unknown, so `--accept-new` cannot overwrite it.
fn known_host_status(
    host: &str,
    port: u16,
    key: &russh::keys::PublicKey,
    path: &Path,
) -> Result<KnownHostStatus> {
    match std::fs::File::open(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(KnownHostStatus::Unknown);
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    }

    let recorded = russh::keys::known_hosts::known_host_keys_path(host, port, path)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    if recorded.is_empty() {
        return Ok(KnownHostStatus::Unknown);
    }
    if recorded.iter().any(|(_, recorded)| recorded == key) {
        return Ok(KnownHostStatus::Matched);
    }
    if recorded
        .iter()
        .any(|(_, recorded)| recorded.algorithm() == key.algorithm())
    {
        return Ok(KnownHostStatus::KeyChanged);
    }

    Ok(KnownHostStatus::AlgorithmMismatch)
}

/// Client handler that enforces the host-key policy during the handshake.
pub(crate) struct Handler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    accept_new: bool,
}

impl client::Handler for Handler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool> {
        let key = server_public_key.public_key();
        let host = self.host.clone();
        let known_hosts = self.known_hosts.clone();
        let port = self.port;
        let check_key = key.clone();
        let status = tokio::task::spawn_blocking(move || {
            known_host_status(&host, port, &check_key, &known_hosts)
        })
        .await
        .context("failed to check the known_hosts file")??;

        match decide_host_key(status, self.accept_new) {
            HostKeyDecision::Accept => Ok(true),
            HostKeyDecision::Learn => {
                let host = self.host.clone();
                let known_hosts = self.known_hosts.clone();
                let port = self.port;
                tokio::task::spawn_blocking(move || {
                    russh::keys::known_hosts::learn_known_hosts_path(
                        &host,
                        port,
                        &key,
                        &known_hosts,
                    )
                })
                .await
                .context("failed to record the host key")?
                .with_context(|| {
                    format!(
                        "failed to record the host key in {}",
                        self.known_hosts.display()
                    )
                })?;
                println!("Learned the host key for {}:{}", self.host, self.port);
                Ok(true)
            }
            HostKeyDecision::RejectUnknown => bail!(
                "the authenticity of {}:{} can't be established: no key in {}. \
                 Connect once with ssh to record it, or pass --accept-new.",
                self.host,
                self.port,
                self.known_hosts.display()
            ),
            HostKeyDecision::RejectChanged => bail!(
                "the host key for {}:{} does not match the one recorded in {}. \
                 If the device was reflashed, remove the stale entry with `ssh-keygen -R {}`.",
                self.host,
                self.port,
                self.known_hosts.display(),
                self.host
            ),
        }
    }
}

/// Opens an authenticated connection to the device.
pub(crate) async fn connect(settings: &Settings) -> Result<Handle<Handler>> {
    let host = settings.host().to_string();
    let port = settings.port();
    let handler = Handler {
        host: host.clone(),
        port,
        known_hosts: settings.known_hosts.clone(),
        accept_new: settings.accept_new,
    };

    let handshake = async {
        let stream = settings
            .config
            .stream()
            .await
            .with_context(|| format!("failed to reach {host}:{port}"))?;
        client::connect_stream(Arc::new(client_config()), stream, handler)
            .await
            .with_context(|| format!("SSH handshake with {host}:{port} failed"))
    };

    let mut handle = tokio::time::timeout(CONNECT_TIMEOUT, handshake)
        .await
        .map_err(|_| {
            anyhow!(
                "timed out connecting to {host}:{port} within {}s",
                CONNECT_TIMEOUT.as_secs()
            )
        })??;

    authenticate(
        &mut handle,
        &settings.user,
        &settings.identities,
        settings.identities_only,
    )
    .await?;
    Ok(handle)
}

/// Client defaults with keepalives enabled so a dead peer is noticed.
fn client_config() -> client::Config {
    client::Config {
        keepalive_interval: Some(KEEPALIVE_INTERVAL),
        keepalive_max: 3,
        ..Default::default()
    }
}

/// Authenticates with the first method the server accepts.
///
/// Configured key files run first, then `none` and the password fallback, and
/// only then ssh-agent identities. That order keeps a large agent from using
/// up the server's auth attempts before a working method is reached.
async fn authenticate(
    handle: &mut Handle<Handler>,
    user: &str,
    identities: &[PathBuf],
    identities_only: bool,
) -> Result<()> {
    let mut tried = Vec::new();

    tried.push("key files");
    let (authenticated, encrypted, configured) = try_key_files(handle, user, identities).await?;
    if authenticated {
        return Ok(());
    }

    tried.push("none");
    if handle
        .authenticate_none(user)
        .await
        .is_ok_and(|auth| auth.success())
    {
        return Ok(());
    }

    tried.push("password");
    let password = std::env::var("KOBO_PASSWORD").unwrap_or_default();
    if handle
        .authenticate_password(user, password.as_str())
        .await
        .is_ok_and(|auth| auth.success())
    {
        return Ok(());
    }

    if std::env::var_os("SSH_AUTH_SOCK").is_some() {
        tried.push("ssh-agent");
        if let Ok(mut agent) = AgentClient::connect_env().await
            && try_agent(handle, &mut agent, user, &configured, identities_only).await?
        {
            return Ok(());
        }
    }

    let message = auth_failure_message(user, &tried, &encrypted);
    bail!("{message}")
}

/// Builds the final authentication failure, naming skipped encrypted keys.
fn auth_failure_message(user: &str, tried: &[&str], encrypted: &[String]) -> String {
    let encrypted_hint = if encrypted.is_empty() {
        String::new()
    } else {
        format!(
            " Passphrase-protected keys were skipped ({}); load them into an agent with ssh-add.",
            encrypted.join(", ")
        )
    };
    format!(
        "authentication as {user} failed; tried {}.{encrypted_hint} \
         Set KOBO_PASSWORD for password login, or load a key into ssh-agent.",
        tried.join(", ")
    )
}

/// Tries the agent identities, configured ones before unrelated ones.
///
/// Binding the agent to the session only matters for agent forwarding, so a
/// rejected binding does not stop key signing. With `identities_only`, agent
/// keys that do not match a configured identity are skipped entirely.
async fn try_agent<S>(
    handle: &mut Handle<Handler>,
    agent: &mut AgentClient<S>,
    user: &str,
    configured: &[russh::keys::PublicKey],
    identities_only: bool,
) -> Result<bool>
where
    S: AgentStream + Send + Unpin,
{
    if let Some(binding) = handle.agent_session_binding() {
        let _ = agent.bind_session(binding, false).await;
    }

    let Ok(identities) = agent.request_identities().await else {
        return Ok(false);
    };

    let matches_configured =
        |identity: &russh::keys::agent::AgentIdentity| configured.contains(&identity.public_key());
    let mut ordered: Vec<_> = identities
        .iter()
        .filter(|identity| matches_configured(identity))
        .collect();
    if !identities_only {
        ordered.extend(
            identities
                .iter()
                .filter(|identity| !matches_configured(identity)),
        );
    }

    for identity in ordered {
        let hash = handle.best_supported_rsa_hash().await?.flatten();
        let result = handle
            .authenticate_publickey_with(user, identity.public_key().into_owned(), hash, agent)
            .await;
        if matches!(result, Ok(auth) if auth.success()) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Tries each configured key file in order.
///
/// Returns whether one authenticated, the paths skipped because they are
/// passphrase-protected, and the public keys of configured identities so the
/// agent pass can try those before unrelated keys.
async fn try_key_files(
    handle: &mut Handle<Handler>,
    user: &str,
    identities: &[PathBuf],
) -> Result<(bool, Vec<String>, Vec<russh::keys::PublicKey>)> {
    let mut encrypted = Vec::new();
    let mut configured = Vec::new();

    for path in identities {
        let key_path = path.clone();
        let loaded =
            tokio::task::spawn_blocking(move || russh::keys::load_secret_key(&key_path, None))
                .await
                .context("failed to load a private key")?;
        let key = match loaded {
            Ok(key) => key,
            Err(russh::keys::Error::KeyIsEncrypted) => {
                match configured_public_key(path).await? {
                    Some(public) => configured.push(public),
                    None => encrypted.push(path.display().to_string()),
                }
                continue;
            }
            Err(_) => {
                if let Some(public) = configured_public_key(path).await? {
                    configured.push(public);
                }
                continue;
            }
        };

        configured.push(key.public_key().clone());
        let hash = handle.best_supported_rsa_hash().await?.flatten();
        let auth = handle
            .authenticate_publickey(
                user,
                russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash),
            )
            .await?;
        if auth.success() {
            return Ok((true, encrypted, configured));
        }
    }

    Ok((false, encrypted, configured))
}

/// Loads the public key beside a configured identity file, off the async pool.
async fn configured_public_key(path: &Path) -> Result<Option<russh::keys::PublicKey>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        russh::keys::load_public_key(public_key_path(&path))
            .or_else(|_| russh::keys::load_public_key(&path))
            .ok()
    })
    .await
    .context("failed to load a public key")
}

/// Path of the public key next to a private key file.
fn public_key_path(path: &Path) -> PathBuf {
    let mut public = path.as_os_str().to_owned();
    public.push(".pub");
    PathBuf::from(public)
}

/// Captured output of a remote command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<u32>,
}

/// Runs `command` on the device and collects stdout, stderr, and the status.
pub(crate) async fn exec(handle: &mut Handle<Handler>, command: &str) -> Result<ExecOutput> {
    let mut channel = handle.channel_open_session().await?;
    channel.exec(true, command).await?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_status = None;
    while let Some(message) = channel.wait().await {
        match message {
            russh::ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            russh::ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
            russh::ChannelMsg::ExitStatus {
                exit_status: status,
            } => exit_status = Some(status),
            russh::ChannelMsg::Failure => {
                let _ = channel.close().await;
                bail!("the server rejected the command request: {command}");
            }
            _ => {}
        }
    }

    Ok(ExecOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_status,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn config_file(contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file
    }

    fn options(config: Option<&Path>, ip: &str) -> SshOptions {
        SshOptions {
            ip: ip.to_string(),
            port: None,
            user: None,
            identity: Vec::new(),
            identities_only: false,
            known_hosts: None,
            accept_new: false,
            ssh_config: config.map(Path::to_path_buf),
        }
    }

    #[tokio::test]
    async fn cli_overrides_ssh_config() {
        let file = config_file(
            "Host kobo\n  User fromconfig\n  Port 2222\n  IdentityFile /config/key\n  \
             UserKnownHostsFile /config/known_hosts\n  StrictHostKeyChecking no\n",
        );
        let mut opts = options(Some(file.path()), "kobo");
        opts.port = Some(2200);
        opts.user = Some("cliuser".to_string());
        opts.identity = vec![PathBuf::from("/cli/key")];
        opts.known_hosts = Some(PathBuf::from("/cli/known_hosts"));

        let settings = resolve(&opts).await.unwrap();

        assert_eq!(settings.host(), "kobo");
        assert_eq!(settings.port(), 2200);
        assert_eq!(settings.user, "cliuser");
        assert_eq!(settings.known_hosts, PathBuf::from("/cli/known_hosts"));
        assert!(settings.accept_new);
        assert_eq!(
            &settings.identities[..2],
            &[PathBuf::from("/cli/key"), PathBuf::from("/config/key")]
        );
    }

    #[tokio::test]
    async fn ssh_config_values_apply_without_cli_overrides() {
        let file = config_file(
            "Host kobo\n  User reader\n  Port 2222\n  Hostname 192.168.1.128\n  \
             UserKnownHostsFile /config/known_hosts\n",
        );
        let settings = resolve(&options(Some(file.path()), "kobo")).await.unwrap();

        assert_eq!(settings.host(), "192.168.1.128");
        assert_eq!(settings.port(), 2222);
        assert_eq!(settings.user, "reader");
        assert_eq!(settings.known_hosts, PathBuf::from("/config/known_hosts"));
        assert!(!settings.accept_new);
    }

    #[tokio::test]
    async fn global_directives_before_the_first_host_are_honoured() {
        let file = config_file(
            "CanonicalizeHostname always\n\
             StrictHostKeyChecking accept-new\n\
             \n\
             Host kobo\n  User reader\n  Port 2222\n  Hostname 192.168.1.128\n",
        );

        let settings = resolve(&options(Some(file.path()), "kobo")).await.unwrap();

        assert_eq!(settings.user, "reader");
        assert_eq!(settings.port(), 2222);
        assert_eq!(settings.host(), "192.168.1.128");
        assert!(settings.accept_new, "accept-new must learn unknown keys");
    }

    #[tokio::test]
    async fn match_blocks_are_refused() {
        let file = config_file("Match host kobo\n  User root\n");

        let error = resolve(&options(Some(file.path()), "kobo"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("Match"), "{error}");
    }

    #[tokio::test]
    async fn defaults_apply_without_config_or_overrides() {
        let file = config_file("");
        let settings = resolve(&options(Some(file.path()), "192.168.1.128"))
            .await
            .unwrap();

        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(settings.host(), "192.168.1.128");
        assert_eq!(settings.port(), 22);
        assert_eq!(settings.user, "root");
        assert_eq!(settings.known_hosts, home.join(".ssh/known_hosts"));
        assert!(!settings.accept_new);
        assert_eq!(settings.identities[0], home.join(".ssh/id_ed25519"));
    }

    #[tokio::test]
    async fn missing_config_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing");
        let settings = resolve(&options(Some(&path), "kobo")).await.unwrap();

        assert_eq!(settings.host(), "kobo");
        assert_eq!(settings.user, "root");
    }

    #[tokio::test]
    async fn accept_new_flag_overrides_a_strict_config() {
        let file = config_file("Host kobo\n  StrictHostKeyChecking yes\n");
        let mut opts = options(Some(file.path()), "kobo");
        opts.accept_new = true;

        assert!(resolve(&opts).await.unwrap().accept_new);
    }

    #[tokio::test]
    async fn proxy_jump_is_refused() {
        let file = config_file("Host kobo\n  ProxyJump bastion\n");
        let error = resolve(&options(Some(file.path()), "kobo"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("ProxyJump"), "{error}");
    }

    const ED_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIHOo87rwqXnbc3va+uUgKkskUS+hCOWmK7WGKR6uQYKm";
    const ED_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIIPwtO6eMarR44pdCXV4LiVVt/olMnK4PN6g8O1nYbeN";
    const EC_C: &str = "AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBAj2f/5JAzBQS4MDBGwBGCM/fhb0RJ+Wt84mfcsdCS9F5BtdPyQGqapj2kmKGhfm6rV3rmlHQqSwzihfNhUvkRM=";

    fn public_key(base64: &str) -> russh::keys::PublicKey {
        russh::keys::parse_public_key_base64(base64).unwrap()
    }

    fn known_hosts_file(dir: &Path, contents: &str) -> PathBuf {
        let path = dir.join("known_hosts");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn host_key_decisions() {
        assert_eq!(
            decide_host_key(KnownHostStatus::Matched, false),
            HostKeyDecision::Accept
        );
        assert_eq!(
            decide_host_key(KnownHostStatus::Unknown, true),
            HostKeyDecision::Learn
        );
        assert_eq!(
            decide_host_key(KnownHostStatus::Unknown, false),
            HostKeyDecision::RejectUnknown
        );
        assert_eq!(
            decide_host_key(KnownHostStatus::KeyChanged, true),
            HostKeyDecision::RejectChanged
        );
        assert_eq!(
            decide_host_key(KnownHostStatus::AlgorithmMismatch, true),
            HostKeyDecision::RejectChanged
        );
    }

    #[test]
    fn known_host_status_reports_match_change_and_algorithm_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = known_hosts_file(dir.path(), &format!("kobo ssh-ed25519 {ED_A}\n"));

        assert_eq!(
            known_host_status("kobo", 22, &public_key(ED_A), &path).unwrap(),
            KnownHostStatus::Matched
        );
        assert_eq!(
            known_host_status("kobo", 22, &public_key(ED_B), &path).unwrap(),
            KnownHostStatus::KeyChanged
        );
        assert_eq!(
            known_host_status("kobo", 22, &public_key(EC_C), &path).unwrap(),
            KnownHostStatus::AlgorithmMismatch
        );
    }

    #[test]
    fn known_host_status_treats_a_missing_file_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing");

        assert_eq!(
            known_host_status("kobo", 22, &public_key(ED_A), &path).unwrap(),
            KnownHostStatus::Unknown
        );
    }

    #[test]
    fn known_host_status_fails_closed_when_the_file_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::create_dir(&path).unwrap();

        assert!(known_host_status("kobo", 22, &public_key(ED_A), &path).is_err());
    }

    #[test]
    fn directive_accepts_equals_and_tab_separators() {
        assert_eq!(directive("Port=2222").unwrap(), Some(("Port", "2222")));
        assert_eq!(
            directive("\tUser\treader ").unwrap(),
            Some(("User", "reader"))
        );
        assert_eq!(directive("Host = kobo").unwrap(), Some(("Host", "kobo")));
        assert_eq!(directive("# comment").unwrap(), None);
        assert_eq!(directive("").unwrap(), None);
        assert!(directive("ProxyJump").is_err());
        assert!(directive("= kobo").is_err());
    }

    #[test]
    fn auth_failure_lists_skipped_encrypted_keys() {
        let message = auth_failure_message(
            "root",
            &["ssh-agent", "key files"],
            &["/home/me/.ssh/id_rsa".to_string()],
        );

        assert!(
            message.contains("Passphrase-protected keys were skipped (/home/me/.ssh/id_rsa)"),
            "{message}"
        );
        assert!(message.contains("tried ssh-agent, key files"), "{message}");
    }

    #[test]
    fn auth_failure_without_skipped_keys_is_plain() {
        let message = auth_failure_message("root", &["none", "password"], &[]);

        assert!(!message.contains("skipped"), "{message}");
        assert!(message.contains("KOBO_PASSWORD"), "{message}");
    }

    #[tokio::test]
    async fn identity_flag_wins_over_ssh_config() {
        let file = config_file("Host kobo\n  IdentityFile /config/key\n");
        let mut opts = options(Some(file.path()), "kobo");
        opts.identity = vec![PathBuf::from("/cli/key")];

        let settings = resolve(&opts).await.unwrap();

        assert_eq!(
            &settings.identities[..2],
            &[PathBuf::from("/cli/key"), PathBuf::from("/config/key")]
        );
    }

    #[tokio::test]
    async fn proxy_none_is_treated_as_unset() {
        let file = config_file("Host kobo\n  ProxyJump none\n  ProxyCommand none\n");

        let settings = resolve(&options(Some(file.path()), "kobo")).await.unwrap();

        assert_eq!(settings.host(), "kobo");
    }

    #[tokio::test]
    async fn equals_and_tab_directives_are_honoured() {
        let file = config_file(
            "Host=kobo\n\tPort=2222\n\tUser\treader\n\tStrictHostKeyChecking=accept-new\n",
        );

        let settings = resolve(&options(Some(file.path()), "kobo")).await.unwrap();

        assert_eq!(settings.port(), 2222);
        assert_eq!(settings.user, "reader");
        assert!(settings.accept_new);
    }

    #[tokio::test]
    async fn proxy_jump_with_alternate_separators_is_refused() {
        for config in [
            "Host kobo\nProxyJump=bastion\n",
            "Host kobo\nProxyJump\tbastion\n",
            "Host kobo\nProxyJump = bastion\n",
        ] {
            let file = config_file(config);
            let error = resolve(&options(Some(file.path()), "kobo"))
                .await
                .unwrap_err();

            assert!(
                error.to_string().contains("ProxyJump"),
                "{config:?}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn identities_only_flag_is_kept() {
        let file = config_file("");
        let mut opts = options(Some(file.path()), "kobo");
        opts.identities_only = true;

        assert!(resolve(&opts).await.unwrap().identities_only);
    }
}
