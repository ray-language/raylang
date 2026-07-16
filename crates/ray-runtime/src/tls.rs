//! **TLS de producción** (rustls) para el binario transpilado (P2.b, Paso 1).
//!
//! **Modelo BLOQUEANTE** (`rustls::StreamOwned`): el binario nativo usa hilos de SO reales, así que —a
//! diferencia de la VM, que hace I/O no-bloqueante con cesión de fibras— el I/O TLS puede bloquear como
//! el intérprete. Un [`TlsStream`] envuelve la sesión rustls + su `TcpStream`; `read`/`write_all` conducen
//! el handshake automáticamente en la primera I/O.
//!
//! El binario transpilado guarda cada `TlsStream` en su registro de handles (una variante nueva) tras un
//! `Arc<Mutex<…>>` propio: así el I/O TLS bloqueante NO retiene el lock global del registro (varias
//! conexiones concurrentes, cada una en su hilo, no se serializan).
//!
//! Sin la feature `tls`, todo es un *stub* (`Err`/panic inalcanzable): el binario compila sin rustls.

#[cfg(feature = "tls")]
mod imp {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::Arc;

    /// Una conexión TLS bloqueante: sesión rustls + socket, vía `StreamOwned`. Es un enum cliente/servidor
    /// porque `StreamOwned` exige el tipo de sesión CONCRETO (el enum unificado `rustls::Connection` no
    /// cumple los bounds de `Deref`); cada variante concreta sí.
    pub enum TlsStream {
        Client(rustls::StreamOwned<rustls::ClientConnection, TcpStream>),
        Server(rustls::StreamOwned<rustls::ServerConnection, TcpStream>),
    }

    impl TlsStream {
        /// Lee hasta `buf.len()` octetos de texto plano (descifra; conduce el handshake si hace falta).
        /// `Ok(0)` = fin de la conexión.
        pub fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self {
                TlsStream::Client(s) => s.read(buf),
                TlsStream::Server(s) => s.read(buf),
            }
        }
        /// Escribe TODO `data` (cifra) y lo vacía al socket.
        pub fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
            match self {
                TlsStream::Client(s) => {
                    s.write_all(data)?;
                    s.flush()
                }
                TlsStream::Server(s) => {
                    s.write_all(data)?;
                    s.flush()
                }
            }
        }
    }

    /// Config de cliente: raíces de Mozilla + (como curl) los certificados extra de `SSL_CERT_FILE` (una CA
    /// propia o —en pruebas— una autofirmada). Espeja `tls_client_config` de la VM (mismo comportamiento).
    fn client_config() -> Arc<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        if let Ok(path) = std::env::var("SSL_CERT_FILE") {
            use rustls::pki_types::pem::PemObject;
            if let Ok(certs) = rustls::pki_types::CertificateDer::pem_file_iter(&path) {
                for cert in certs.flatten() {
                    let _ = roots.add(cert);
                }
            }
        }
        Arc::new(rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth())
    }

    /// Config de servidor a partir de los PEM de la cadena de certificados y la clave privada. No se cachea
    /// (cada servidor puede tener su propio certificado). Espeja `tls_server_config` de la VM.
    fn server_config(cert_pem: &str, key_pem: &str) -> Result<Arc<rustls::ServerConfig>, String> {
        use rustls::pki_types::pem::PemObject;
        let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
            rustls::pki_types::CertificateDer::pem_slice_iter(cert_pem.as_bytes())
                .collect::<Result<_, _>>()
                .map_err(|e| format!("invalid certificate: {e}"))?;
        if certs.is_empty() {
            return Err("the PEM contains no certificate".to_string());
        }
        let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
            .map_err(|e| format!("invalid private key: {e}"))?;
        let cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(cfg))
    }

    /// Conecta un TCP a `host:port` y lo envuelve en una sesión TLS de cliente (verifica el cert vía SNI).
    /// El handshake ocurre en la primera I/O (`StreamOwned` lo conduce).
    pub fn connect(host: &str, port: i64) -> Result<TlsStream, String> {
        let sn = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|_| format!("invalid server name for TLS: {host}"))?;
        let conn = rustls::ClientConnection::new(client_config(), sn).map_err(|e| e.to_string())?;
        let sock = TcpStream::connect((host, port as u16)).map_err(|e| e.to_string())?;
        Ok(TlsStream::Client(rustls::StreamOwned::new(conn, sock)))
    }

    /// Como [`connect`] pero ofreciendo ALPN `h2` (HTTP/2). Completa el handshake (bloqueante) para poder
    /// exigir que el servidor negocie `h2`; si no, error.
    pub fn connect_h2(host: &str, port: i64) -> Result<TlsStream, String> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        if let Ok(path) = std::env::var("SSL_CERT_FILE") {
            use rustls::pki_types::pem::PemObject;
            if let Ok(certs) = rustls::pki_types::CertificateDer::pem_file_iter(&path) {
                for cert in certs.flatten() {
                    let _ = roots.add(cert);
                }
            }
        }
        let mut cfg = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        cfg.alpn_protocols = vec![b"h2".to_vec()];
        let sn = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|_| format!("invalid server name for TLS: {host}"))?;
        let mut conn = rustls::ClientConnection::new(Arc::new(cfg), sn).map_err(|e| e.to_string())?;
        let mut sock = TcpStream::connect((host, port as u16)).map_err(|e| e.to_string())?;
        while conn.is_handshaking() {
            conn.complete_io(&mut sock).map_err(|e| e.to_string())?;
        }
        match conn.alpn_protocol() {
            Some(p) if p == b"h2" => {}
            _ => return Err("the server did not negotiate HTTP/2 (ALPN 'h2')".to_string()),
        }
        Ok(TlsStream::Client(rustls::StreamOwned::new(conn, sock)))
    }

    /// Envuelve un `TcpStream` YA conectado en una sesión TLS de cliente (STARTTLS), verificando el cert
    /// contra `host`. El simétrico de [`accept`].
    pub fn upgrade(sock: TcpStream, host: &str) -> Result<TlsStream, String> {
        let sn = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|_| format!("invalid server name for TLS: {host}"))?;
        let conn = rustls::ClientConnection::new(client_config(), sn).map_err(|e| e.to_string())?;
        Ok(TlsStream::Client(rustls::StreamOwned::new(conn, sock)))
    }

    /// Envuelve un `TcpStream` ya aceptado en una sesión TLS de servidor con el certificado/clave PEM.
    pub fn accept(sock: TcpStream, cert_pem: &str, key_pem: &str) -> Result<TlsStream, String> {
        let cfg = server_config(cert_pem, key_pem)?;
        let conn = rustls::ServerConnection::new(cfg).map_err(|e| e.to_string())?;
        Ok(TlsStream::Server(rustls::StreamOwned::new(conn, sock)))
    }
}

#[cfg(feature = "tls")]
pub use imp::{accept, connect, connect_h2, upgrade, TlsStream};

// --- Stubs sin la feature `tls` (el binario compila sin rustls; el consumidor no los alcanza) ---
#[cfg(not(feature = "tls"))]
mod stub {
    use std::net::TcpStream;

    /// Sin la feature `tls`, `TlsStream` es un tipo vacío inalcanzable (el consumidor lo gatea por checker).
    pub struct TlsStream(std::convert::Infallible);
    impl TlsStream {
        pub fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0 {}
        }
        pub fn write_all(&mut self, _data: &[u8]) -> std::io::Result<()> {
            match self.0 {}
        }
    }
    const UNAVAIL: &str = "TLS not available (build without the 'tls' feature)";
    pub fn connect(_host: &str, _port: i64) -> Result<TlsStream, String> { Err(UNAVAIL.to_string()) }
    pub fn connect_h2(_host: &str, _port: i64) -> Result<TlsStream, String> { Err(UNAVAIL.to_string()) }
    pub fn upgrade(_sock: TcpStream, _host: &str) -> Result<TlsStream, String> { Err(UNAVAIL.to_string()) }
    pub fn accept(_sock: TcpStream, _cert_pem: &str, _key_pem: &str) -> Result<TlsStream, String> {
        Err(UNAVAIL.to_string())
    }
}

#[cfg(not(feature = "tls"))]
pub use stub::{accept, connect, connect_h2, upgrade, TlsStream};
