use anyhow::Result;
use rustls::{
    client::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier, WebPkiVerifier},
    internal::msgs::handshake::DigitallySignedStruct,
    Certificate, ClientConfig, Error, RootCertStore, ServerName, SignatureScheme,
};
use std::{fs::File, io::Read, path::Path, sync::Arc, time::SystemTime};

struct SpecificResolver {
    certificate:       Certificate,
    internal_verifier: WebPkiVerifier,
}

impl ServerCertVerifier for SpecificResolver {
    fn verify_server_cert(
        &self,
        end_entity: &Certificate,
        intermediates: &[Certificate],
        server_name: &ServerName,
        scts: &mut dyn Iterator<Item = &[u8]>,
        ocsp_response: &[u8],
        now: SystemTime,
    ) -> Result<ServerCertVerified, Error> {
        if end_entity != &self.certificate {
            return Err(Error::General("Not the expected certificate".to_string()));
        }
        self.internal_verifier.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            scts,
            ocsp_response,
            now,
        )
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &Certificate,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        if cert != &self.certificate {
            return Err(Error::General("Not the expected certificate".to_string()));
        }
        self.internal_verifier.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &Certificate,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        if cert != &self.certificate {
            return Err(Error::General("Not the expected certificate".to_string()));
        }
        self.internal_verifier.verify_tls12_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.internal_verifier.supported_verify_schemes()
    }

    fn request_scts(&self) -> bool { self.internal_verifier.request_scts() }
}

pub fn get_client_with_specific_certificate(location: &Path) -> Result<reqwest::Client> {
    let mut buf = Vec::new();
    File::open(location)?.read_to_end(&mut buf)?;
    let cert = Certificate(buf);
    let mut roots = RootCertStore::empty();
    roots.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
        rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject,
            ta.spki,
            ta.name_constraints,
        )
    }));
    let tls = ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(SpecificResolver {
            certificate:       cert,
            internal_verifier: WebPkiVerifier::new(roots, None),
        }))
        .with_no_client_auth();
    Ok(reqwest::Client::builder().use_preconfigured_tls(tls).build()?)
}
