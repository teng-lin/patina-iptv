//! Cast receivers use device certificates outside the Web PKI.
use std::sync::Arc;
use tokio_rustls::{
    TlsConnector,
    rustls::{
        self,
        client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        crypto::{self, ring},
        pki_types::{CertificateDer, ServerName, SubjectPublicKeyInfoDer, UnixTime},
    },
};
use x509_cert::der::{Decode, Encode};

#[derive(Debug)]
struct ReceiverCertificate;
// Cast devices may present v1 certificates. Extract the SPKI independently
// of Web PKI's v3 end-entity policy, then check the TLS handshake signature.
fn public_key(
    cert: &CertificateDer<'_>,
) -> Result<SubjectPublicKeyInfoDer<'static>, rustls::Error> {
    let bad = || rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding);
    let parsed = x509_cert::Certificate::from_der(cert.as_ref()).map_err(|_| bad())?;
    Ok(parsed
        .tbs_certificate
        .subject_public_key_info
        .to_der()
        .map_err(|_| bad())?
        .into())
}
impl ServerCertVerifier for ReceiverCertificate {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        sig: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let spki = public_key(cert)?;
        let key = webpki::RawPublicKeyEntity::try_from(&spki).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        })?;
        let algorithms = ring::default_provider().signature_verification_algorithms;
        if let Some((_, supported)) = algorithms
            .mapping
            .iter()
            .find(|(scheme, _)| *scheme == sig.scheme)
            && supported
                .iter()
                .any(|alg| key.verify_signature(*alg, msg, sig.signature()).is_ok())
        {
            Ok(HandshakeSignatureValid::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadSignature,
            ))
        }
    }
    fn verify_tls13_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        sig: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls13_signature_with_raw_key(
            msg,
            &public_key(cert)?,
            sig,
            &ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
pub(crate) fn connector() -> Result<TlsConnector, rustls::Error> {
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ReceiverCertificate))
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}
