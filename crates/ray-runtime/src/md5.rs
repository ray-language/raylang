//! MD5 (RFC 1321) en Rust puro, sin dependencias (rayblob #282). LEGADO: solo para hablar con
//! protocolos que lo exigen (ETag de S3, `Content-MD5`, digest auth); para lo nuevo, SHA-256.
//!
//! Vive aquí (y no tras la feature `crypto`, que es ring y no trae MD5) porque lo usan los tres
//! motores: la VM y el intérprete lo enlazan desde `ray-runtime`; el build wasm32 del playground,
//! que no enlaza el crate, incluye este mismo archivo por `#[path]` desde `src/builtins.rs`. Una
//! sola fuente de verdad: un cambio aquí vale para todos.
//!
//! Incremental (`Md5::update` tantas veces como haga falta + `finalize`): el estado son 4 words, un
//! buffer de hasta 63 octetos y la longitud total — nada proporcional a la entrada.

/// Estado de un MD5 en curso.
#[derive(Clone)]
pub struct Md5 {
    state: [u32; 4],
    buf: Vec<u8>,
    len: u64,
}

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14,
    20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6,
    10, 15, 21,
];

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501, 0x698098d8,
    0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340,
    0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87,
    0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
    0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039,
    0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92,
    0xffeff47d, 0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
    0xeb86d391,
];

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Md5 {
    /// Un hasher vacío.
    pub fn new() -> Self {
        Md5 { state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476], buf: Vec::with_capacity(64), len: 0 }
    }

    /// Alimenta `data` (en cualquier troceo: el resultado solo depende de la concatenación).
    pub fn update(&mut self, data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u64);
        let mut rest = data;
        if !self.buf.is_empty() {
            let take = (64 - self.buf.len()).min(rest.len());
            self.buf.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.buf.len() == 64 {
                let block: [u8; 64] = self.buf[..].try_into().expect("64-byte block");
                compress(&mut self.state, &block);
                self.buf.clear();
            }
        }
        while rest.len() >= 64 {
            let block: [u8; 64] = rest[..64].try_into().expect("64-byte block");
            compress(&mut self.state, &block);
            rest = &rest[64..];
        }
        self.buf.extend_from_slice(rest);
    }

    /// Cierra y devuelve el digest (16 octetos).
    pub fn finalize(mut self) -> [u8; 16] {
        let bit_len = self.len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.buf.len() != 56 {
            self.update(&[0]);
        }
        // Los 8 octetos de longitud entran directos (no cuentan para `len`, que ya no se usa).
        self.buf.extend_from_slice(&bit_len.to_le_bytes());
        let block: [u8; 64] = self.buf[..].try_into().expect("64-byte block");
        compress(&mut self.state, &block);
        let mut out = [0u8; 16];
        for (i, w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

/// El digest de `data` en una pasada.
pub fn digest(data: &[u8]) -> [u8; 16] {
    let mut h = Md5::new();
    h.update(data);
    h.finalize()
}

fn compress(state: &mut [u32; 4], block: &[u8; 64]) {
    let mut m = [0u32; 16];
    for (i, w) in m.iter_mut().enumerate() {
        *w = u32::from_le_bytes([block[i * 4], block[i * 4 + 1], block[i * 4 + 2], block[i * 4 + 3]]);
    }
    let (mut a, mut b, mut c, mut d) = (state[0], state[1], state[2], state[3]);
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(f.rotate_left(S[i]));
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn rfc_1321_vectors() {
        assert_eq!(hex(&digest(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(&digest(b"a")), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(hex(&digest(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hex(&digest(b"message digest")), "f96b697d7cb7938d525a2f31aaf161d0");
        assert_eq!(hex(&digest(b"abcdefghijklmnopqrstuvwxyz")), "c3fcd3d76192e4007dfb496cca67e13b");
        assert_eq!(
            hex(&digest(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890")),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn incremental_matches_one_shot_for_every_chunking() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        let want = digest(&data);
        for chunk in [1usize, 3, 55, 56, 63, 64, 65, 100, 999] {
            let mut h = Md5::new();
            for part in data.chunks(chunk) {
                h.update(part);
            }
            assert_eq!(h.finalize(), want, "chunk {chunk}");
        }
    }
}
