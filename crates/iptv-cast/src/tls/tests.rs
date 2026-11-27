use super::*;

#[test]
fn a_truncated_certificate_does_not_yield_a_public_key() {
    let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let der = certificate.cert.der().as_ref();
    let truncated = CertificateDer::from(&der[..der.len() / 2]);

    assert_eq!(
        public_key(&truncated),
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::BadEncoding
        ))
    );
}
