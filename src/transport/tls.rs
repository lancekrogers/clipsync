#![deny(warnings, clippy::all)]
//! TLS 1.3 with RFC 7250 raw Ed25519 keys. Pins identify devices; rustls verifies
//! CertificateVerify signatures and owns the key exchange and record protection.
use crate::auth::{Authenticator, KeyType, PublicKey};
use anyhow::{bail, Result};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{
        CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, SubjectPublicKeyInfoDer,
        UnixTime,
    },
    server::danger::{ClientCertVerified, ClientCertVerifier},
    DigitallySignedStruct, DistinguishedName, Error, SignatureScheme,
};
use std::sync::Arc;

const ED25519_SPKI: &[u8] = b"\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00";
pub fn spki(key: &PublicKey) -> Result<Vec<u8>> {
    if key.key_type != KeyType::Ed25519 || key.key_data.len() != 32 {
        bail!("Only Ed25519 identities are supported");
    }
    Ok([ED25519_SPKI, &key.key_data].concat())
}
pub fn public_key(der: &[u8]) -> Result<PublicKey> {
    if der.len() != 44 || !der.starts_with(ED25519_SPKI) {
        bail!("Invalid Ed25519 identity");
    }
    Ok(PublicKey::new(KeyType::Ed25519, der[12..].to_vec()))
}
pub fn node_id(key: &PublicKey) -> uuid::Uuid {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(&key.key_data);
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Uuid::from_bytes(bytes)
}
#[derive(Debug)]
struct Pins(Vec<Vec<u8>>);
impl Pins {
    fn check(&self, key: &CertificateDer<'_>, chain: &[CertificateDer<'_>]) -> Result<(), Error> {
        if chain.is_empty() && self.0.iter().any(|pin| pin.as_slice() == key.as_ref()) {
            Ok(())
        } else {
            Err(Error::General("Peer identity is not authorized".into()))
        }
    }
    fn signature(
        &self,
        msg: &[u8],
        key: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature_with_raw_key(
            msg,
            &SubjectPublicKeyInfoDer::from(key.as_ref()),
            sig,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
}
impl ServerCertVerifier for Pins {
    fn verify_server_cert(
        &self,
        key: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        self.check(key, chain)?;
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.3 required".into()))
    }
    fn verify_tls13_signature(
        &self,
        msg: &[u8],
        key: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.signature(msg, key, sig)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}
impl ClientCertVerifier for Pins {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        key: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        self.check(key, chain)?;
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.3 required".into()))
    }
    fn verify_tls13_signature(
        &self,
        msg: &[u8],
        key: &CertificateDer<'_>,
        sig: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.signature(msg, key, sig)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

pub async fn configs(
    auth: &dyn Authenticator,
) -> Result<(Arc<rustls::ClientConfig>, Arc<rustls::ServerConfig>)> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(auth.identity_pkcs8().await?));
    let signer = provider.key_provider.load_private_key(private)?;
    let public = spki(&auth.get_public_key().await?)?;
    let identity = Arc::new(rustls::sign::CertifiedKey::new(
        vec![CertificateDer::from(public)],
        signer,
    ));
    let pins = Arc::new(Pins(
        auth.trusted_keys()
            .await?
            .iter()
            .map(spki)
            .collect::<Result<_>>()?,
    ));
    let mut client = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(pins.clone())
        .with_client_cert_resolver(Arc::new(
            rustls::client::AlwaysResolvesClientRawPublicKeys::new(identity.clone()),
        ));
    // Re-check authorization on every connection; no tickets or 0-RTT.
    client.resumption = rustls::client::Resumption::disabled();
    client.alpn_protocols = vec![b"clipsync/2".to_vec()];
    let mut server = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(pins)
        .with_cert_resolver(Arc::new(
            rustls::server::AlwaysResolvesServerRawPublicKeys::new(identity),
        ));
    server.send_tls13_tickets = 0;
    server.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    server.alpn_protocols = vec![b"clipsync/2".to_vec()];
    Ok((Arc::new(client), Arc::new(server)))
}
