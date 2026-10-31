/// Installs the ring CryptoProvider as the process-wide rustls default.
///
/// rustls panics when both `ring` and `aws-lc-rs` are linked and no provider
/// has been installed. [`crate::http::Client::new`] installs one, so a client
/// built before application startup — each nextest process, for example —
/// does not panic. Startup calls this as well. A later call is ignored:
/// `install_default` fails once a provider is already installed, and that
/// error is discarded.
pub fn init_crypto_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
}
