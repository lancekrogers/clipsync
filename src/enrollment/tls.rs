//! Enrollment-only TLS 1.3 Ed25519 raw-public-key configs.
//!
//! `UnknownKeyVerifier` accepts any well-formed Ed25519 RPK after rustls verifies
//! the TLS 1.3 CertificateVerify signature. It must **never** be used for
//! production `clipsync/2` sync — that path continues to pin via `tls::configs`.

use crate::{
    auth::{Authenticator, PublicKey},
    transport::tls,
};
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

pub const ENROLL_ALPN: &[u8] = b"clipsync-enroll/1";

#[derive(Debug)]
struct UnknownKeyVerifier;

impl UnknownKeyVerifier {
    fn check_shape(key: &CertificateDer<'_>, chain: &[CertificateDer<'_>]) -> Result<(), Error> {
        if !chain.is_empty() {
            return Err(Error::General(
                "Enrollment rejects certificate chains".into(),
            ));
        }
        tls::public_key(key.as_ref()).map_err(|e| Error::General(e.to_string()))?;
        Ok(())
    }

    fn signature(
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

impl ServerCertVerifier for UnknownKeyVerifier {
    fn verify_server_cert(
        &self,
        key: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Self::check_shape(key, chain)?;
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
        Self::signature(msg, key, sig)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

impl ClientCertVerifier for UnknownKeyVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        key: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        Self::check_shape(key, chain)?;
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
        Self::signature(msg, key, sig)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

/// Build enrollment client/server rustls configs (ALPN `clipsync-enroll/1`).
pub async fn enroll_configs(
    auth: &dyn Authenticator,
) -> Result<(Arc<rustls::ClientConfig>, Arc<rustls::ServerConfig>)> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(auth.identity_pkcs8().await?));
    let signer = provider.key_provider.load_private_key(private)?;
    let public = tls::spki(&auth.get_public_key().await?)?;
    let identity = Arc::new(rustls::sign::CertifiedKey::new(
        vec![CertificateDer::from(public)],
        signer,
    ));
    let verifier = Arc::new(UnknownKeyVerifier);
    let mut client = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_client_cert_resolver(Arc::new(
            rustls::client::AlwaysResolvesClientRawPublicKeys::new(identity.clone()),
        ));
    client.resumption = rustls::client::Resumption::disabled();
    client.alpn_protocols = vec![ENROLL_ALPN.to_vec()];
    let mut server = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_cert_resolver(Arc::new(
            rustls::server::AlwaysResolvesServerRawPublicKeys::new(identity),
        ));
    server.send_tls13_tickets = 0;
    server.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    server.alpn_protocols = vec![ENROLL_ALPN.to_vec()];
    Ok((Arc::new(client), Arc::new(server)))
}

pub fn peer_key_from_common(common: &rustls::CommonState) -> Result<PublicKey> {
    if common.alpn_protocol() != Some(ENROLL_ALPN) {
        bail!("Missing enrollment protocol negotiation");
    }
    let cert = common
        .peer_certificates()
        .and_then(|c| c.first())
        .ok_or_else(|| anyhow::anyhow!("Missing peer identity"))?;
    tls::public_key(cert.as_ref())
}
