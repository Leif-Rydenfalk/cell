use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;

pub fn make_server_config(
    cert: (rustls::Certificate, rustls::PrivateKey),
) -> Result<quinn::ServerConfig> {
    let (cert, key) = cert;
    let mut server_crypto = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)?;

    server_crypto.alpn_protocols = vec![b"cell-v1".to_vec()];

    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(server_crypto));
    let transport = Arc::get_mut(&mut server_config.transport).unwrap();
    transport.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
    transport.keep_alive_interval(Some(Duration::from_secs(5)));

    Ok(server_config)
}

pub fn make_client_config() -> Result<quinn::ClientConfig> {
    // DANGEROUS: Blindly trust all certificates for MVP Hole Punching.
    // In prod, we would verify the Peer's public key against the DHT record.
    struct SkipServerVerification;
    impl rustls::client::ServerCertVerifier for SkipServerVerification {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::Certificate,
            _intermediates: &[rustls::Certificate],
            _server_name: &rustls::ServerName,
            _scts: &mut dyn Iterator<Item = &[u8]>,
            _ocsp_response: &[u8],
            _now: std::time::SystemTime,
        ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::ServerCertVerified::assertion())
        }
    }

    let crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();

    let mut client_config = quinn::ClientConfig::new(Arc::new(crypto));
    let transport = Arc::get_mut(&mut client_config.transport).unwrap();
    transport.keep_alive_interval(Some(Duration::from_secs(5)));

    Ok(client_config)
}

// Generate self-signed cert based on Cell Name
pub fn generate_cert(
    subject_alt_names: Vec<String>,
) -> Result<(rustls::Certificate, rustls::PrivateKey)> {
    let cert = rcgen::generate_simple_self_signed(subject_alt_names)?;
    let key = rustls::PrivateKey(cert.serialize_private_key_der());
    let cert = rustls::Certificate(cert.serialize_der()?);
    Ok((cert, key))
}
