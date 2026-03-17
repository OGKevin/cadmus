/// Installs the ring CryptoProvider as the process-wide rustls default.
///
/// Must be called once at application startup before any TLS connections are
/// made.
pub fn init_crypto_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
}
