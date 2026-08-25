//! M124 — resumen del certificado X.509 del peer (`net.tls_peer_cert`).
//!
//! rustls valida la cadena y las fechas en el handshake, pero no expone los CAMPOS del
//! certificado — y "expira en N días" es EL check de TLS que todo operador quiere (hallazgo de
//! raywatch, IDEAS §70.1). Este módulo extrae del DER lo que un monitor/cliente necesita:
//! subject, issuer, ventana de validez (epoch ms, comparable con `time.now()`) y los SAN.
//!
//! Compartido por los DOS motores (la VM activa la feature `x509` sin arrastrar el TLS de este
//! crate — trae su propio rustls): mismo código → los strings del resumen son byte-idénticos.

#[cfg(feature = "x509")]
mod real {
    /// El resumen de un certificado: lo que un check de expiración/identidad necesita.
    pub struct CertSummary {
        /// El subject como nombre X.500 (`"CN=localhost"`, `"CN=x, O=Acme"`).
        pub subject: String,
        /// El issuer, mismo formato.
        pub issuer: String,
        /// Inicio de validez (notBefore) en epoch ms.
        pub not_before_ms: i64,
        /// Fin de validez (notAfter) en epoch ms — `(not_after_ms - time.now())` = lo que queda.
        pub not_after_ms: i64,
        /// Los Subject Alternative Names: DNS e IPs como strings (lo demás se omite).
        pub san: Vec<String>,
    }

    /// Extrae el resumen del certificado DER. `Err` si el DER no parsea como X.509.
    pub fn cert_summary(der: &[u8]) -> Result<CertSummary, String> {
        use x509_parser::prelude::*;
        let (_, cert) = X509Certificate::from_der(der).map_err(|e| format!("invalid X.509 certificate: {e}"))?;
        let mut san: Vec<String> = Vec::new();
        if let Ok(Some(ext)) = cert.subject_alternative_name() {
            for name in &ext.value.general_names {
                match name {
                    GeneralName::DNSName(d) => san.push((*d).to_string()),
                    GeneralName::IPAddress(b) => match b.len() {
                        4 => san.push(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])),
                        16 => {
                            let mut segs = [0u16; 8];
                            for (i, seg) in segs.iter_mut().enumerate() {
                                *seg = u16::from_be_bytes([b[2 * i], b[2 * i + 1]]);
                            }
                            san.push(std::net::Ipv6Addr::from(segs).to_string());
                        }
                        _ => {}
                    },
                    _ => {} // email/URI/otros: fuera del caso de uso (identidad de servidor)
                }
            }
        }
        Ok(CertSummary {
            subject: cert.subject().to_string(),
            issuer: cert.issuer().to_string(),
            not_before_ms: cert.validity().not_before.timestamp() * 1000,
            not_after_ms: cert.validity().not_after.timestamp() * 1000,
            san,
        })
    }
}

#[cfg(feature = "x509")]
pub use real::{cert_summary, CertSummary};

// Sin la feature: stub que nunca se construye (el llamador ya devolvió su error de feature).
#[cfg(not(feature = "x509"))]
mod stub {
    pub struct CertSummary {
        pub subject: String,
        pub issuer: String,
        pub not_before_ms: i64,
        pub not_after_ms: i64,
        pub san: Vec<String>,
    }

    pub fn cert_summary(_der: &[u8]) -> Result<CertSummary, String> {
        Err("X.509 support is not compiled in (feature 'x509')".to_string())
    }
}

#[cfg(not(feature = "x509"))]
pub use stub::{cert_summary, CertSummary};
