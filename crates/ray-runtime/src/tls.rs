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
        /// La dirección del peer del socket TCP subyacente (M123: `net.peer_addr` sobre un handle TLS).
        pub fn peer_addr(&self) -> std::io::Result<std::net::SocketAddr> {
            match self {
                TlsStream::Client(s) => s.sock.peer_addr(),
                TlsStream::Server(s) => s.sock.peer_addr(),
            }
        }

        /// El certificado del peer en DER (el primero de la cadena), o `None` si no presentó
        /// (lado servidor sin client-auth). M124: la materia prima de `net.tls_peer_cert`.
        pub fn peer_cert_der(&self) -> Option<Vec<u8>> {
            match self {
                TlsStream::Client(s) => s.conn.peer_certificates().and_then(|c| c.first()).map(|c| c.to_vec()),
                TlsStream::Server(s) => s.conn.peer_certificates().and_then(|c| c.first()).map(|c| c.to_vec()),
            }
        }

        /// ¿El handshake sigue en curso? (el certificado del peer solo está tras completarlo).
        fn is_handshaking(&self) -> bool {
            match self {
                TlsStream::Client(s) => s.conn.is_handshaking(),
                TlsStream::Server(s) => s.conn.is_handshaking(),
            }
        }

        /// Una vuelta de `complete_io` (conduce el handshake sobre el socket subyacente).
        fn complete_io_once(&mut self) -> std::io::Result<()> {
            match self {
                TlsStream::Client(s) => s.conn.complete_io(&mut s.sock).map(|_| ()),
                TlsStream::Server(s) => s.conn.complete_io(&mut s.sock).map(|_| ()),
            }
        }

        /// M124: el resumen del certificado del peer (`net.tls_peer_cert`). `tls_connect` deja el
        /// handshake para la primera I/O, así que aquí se CONDUCE si sigue pendiente (acotado a
        /// 10 s): con fibras, WouldBlock aparca la fibra (`wait_ready`); sin fibras el socket es
        /// bloqueante y `complete_io` simplemente bloquea.
        pub fn peer_cert_summary(&mut self) -> Result<crate::x509::CertSummary, String> {
            #[cfg(feature = "fibers")]
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while self.is_handshaking() {
                match self.complete_io_once() {
                    Ok(()) => {}
                    #[cfg(feature = "fibers")]
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        self.wait_ready(Some(deadline)).map_err(|e| {
                            if e.kind() == std::io::ErrorKind::TimedOut {
                                "TLS handshake timeout".to_string()
                            } else {
                                e.to_string()
                            }
                        })?;
                    }
                    Err(e) => return Err(e.to_string()),
                }
            }
            let der = self.peer_cert_der().ok_or_else(|| "the peer presented no certificate".to_string())?;
            crate::x509::cert_summary(&der)
        }

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

    // ─── F4 (arco de concurrencia nativa): I/O TLS apto para FIBRAS ────────────────────────────
    //
    // Con `--fibers` los sockets son NO-bloqueantes: `read`/`write_all` de arriba devolverían
    // WouldBlock (incluido el handshake, que StreamOwned conduce dentro de la primera I/O). Estas
    // variantes esperan readiness y reintentan: en fibra APARCAN (park del reactor), fuera hacen
    // poll(2). La DIRECCIÓN de la espera sale de la sesión rustls (`wants_write`): el handshake
    // alterna lecturas y escrituras, y aparcar por lectura cuando toca escribir interbloquearía.
    #[cfg(feature = "fibers")]
    impl TlsStream {
        fn raw_fd(&self) -> i32 {
            // M182: en Windows el "fd" del reactor es el SOCKET (WSAPoll), truncado a i32 como en
            // el resto del runtime (los handles de Winsock son valores pequeños).
            #[cfg(unix)]
            use std::os::fd::AsRawFd;
            #[cfg(windows)]
            use std::os::windows::io::AsRawSocket;
            match self {
                #[cfg(unix)]
                TlsStream::Client(s) => s.sock.as_raw_fd(),
                #[cfg(unix)]
                TlsStream::Server(s) => s.sock.as_raw_fd(),
                #[cfg(windows)]
                TlsStream::Client(s) => s.sock.as_raw_socket() as i32,
                #[cfg(windows)]
                TlsStream::Server(s) => s.sock.as_raw_socket() as i32,
            }
        }

        fn wants_write(&self) -> bool {
            match self {
                TlsStream::Client(s) => s.conn.wants_write(),
                TlsStream::Server(s) => s.conn.wants_write(),
            }
        }

        /// Marca el socket subyacente como (no-)bloqueante. Lo llama el runtime emitido con
        /// `--fibers` justo tras crear la sesión (los sockets aceptados/upgradeados ya vienen
        /// no-bloqueantes de F2; esto cubre los de `connect`/`connect_h2`, que nacen aquí).
        pub fn set_nonblocking(&self, nb: bool) -> std::io::Result<()> {
            match self {
                TlsStream::Client(s) => s.sock.set_nonblocking(nb),
                TlsStream::Server(s) => s.sock.set_nonblocking(nb),
            }
        }

        /// Como [`TlsStream::read`], esperando readiness en WouldBlock. `timeout_ms > 0` acota la
        /// operación completa (el read-timeout M56.4); vencido → `ErrorKind::TimedOut` (el
        /// llamador lo normaliza a "read timeout", byte-idéntico a la VM).
        pub fn read_wait(&mut self, buf: &mut [u8], timeout_ms: i64) -> std::io::Result<usize> {
            let deadline = if timeout_ms > 0 {
                Some(std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64))
            } else {
                None
            };
            loop {
                match self.read(buf) {
                    Ok(n) => return Ok(n),
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        self.wait_ready(deadline)?;
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        /// Como [`TlsStream::write_all`], esperando readiness en WouldBlock (sin plazo: el modelo
        /// de escritura no tiene timeout, como en TCP plano).
        pub fn write_all_wait(&mut self, data: &[u8]) -> std::io::Result<()> {
            let mut off = 0;
            loop {
                let r = match self {
                    TlsStream::Client(s) => {
                        use std::io::Write;
                        s.write(&data[off..]).and_then(|n| { s.flush()?; Ok(n) })
                    }
                    TlsStream::Server(s) => {
                        use std::io::Write;
                        s.write(&data[off..]).and_then(|n| { s.flush()?; Ok(n) })
                    }
                };
                match r {
                    Ok(n) => {
                        off += n;
                        if off >= data.len() {
                            return Ok(());
                        }
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        self.wait_ready(None)?;
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        /// Espera la readiness que la sesión NECESITA (escritura si rustls tiene datos pendientes
        /// de vaciar; lectura si espera del peer). Vencido el plazo → `TimedOut`.
        fn wait_ready(&self, deadline: Option<std::time::Instant>) -> std::io::Result<()> {
            let fd = self.raw_fd();
            let timed_out = if self.wants_write() {
                // La escritura no lleva plazo (como TCP plano): espera sin límite.
                crate::fibers::wait_writable(fd);
                false
            } else {
                let ms = match deadline {
                    None => 0,
                    Some(d) => {
                        let rem = d.saturating_duration_since(std::time::Instant::now()).as_millis() as i64;
                        if rem <= 0 {
                            return Err(std::io::Error::from(std::io::ErrorKind::TimedOut));
                        }
                        rem
                    }
                };
                crate::fibers::wait_readable_timeout(fd, ms)
            };
            if timed_out {
                return Err(std::io::Error::from(std::io::ErrorKind::TimedOut));
            }
            Ok(())
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
        pub fn peer_addr(&self) -> std::io::Result<std::net::SocketAddr> {
            match self.0 {}
        }
        pub fn peer_cert_der(&self) -> Option<Vec<u8>> {
            match self.0 {}
        }
        pub fn peer_cert_summary(&mut self) -> Result<crate::x509::CertSummary, String> {
            match self.0 {}
        }
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
