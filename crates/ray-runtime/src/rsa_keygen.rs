//! M348 — **generación de claves RSA** sin dependencias nuevas: `num-bigint` (ya en el runtime por
//! `std/bigint`) para la aritmética y el CSPRNG de ring para la entropía. El resultado es **PKCS#8
//! DER**, el formato que `ring::signature::RsaKeyPair::from_pkcs8` acepta — así `rsa_generate` se
//! encadena con `rsa_pkcs1_sign`/`rsa_public_key_of` y con `openssl` sin conversión.
//!
//! Esquema (FIPS 186-4 §B.3.3 simplificado, el de OpenSSL): dos primos de `bits/2` con los dos bits
//! altos a 1 (para que `n` tenga exactamente `bits`) y el bajo a 1, probados con una criba de primos
//! pequeños y Miller-Rabin (`MR_ROUNDS` rondas: error < 2^-128 para 1024 bits), `e = 65537` con
//! `gcd(e, p-1) = gcd(e, q-1) = 1`, `d = e^-1 mod lcm(p-1, q-1)`, y los parámetros CRT que ring exige
//! (`dP`, `dQ`, `qInv`), con `p > q` (ring lo requiere). La aritmética NO es de tiempo constante,
//! pero la generación de una clave es una operación única cuyo tiempo no se correlaciona con un
//! secreto que un atacante remoto pueda observar por petición; la clave resultante se usa luego
//! solo a través de ring.

#[cfg(all(feature = "crypto", feature = "bigint"))]
pub fn generate_pkcs8(bits: u32) -> Result<Vec<u8>, String> {
    use num_bigint::BigUint;
    use num_integer::Integer;
    use num_traits::{One, Zero};
    if !(2048..=4096).contains(&bits) || bits % 64 != 0 {
        return Err(format!("rsa_generate: bits must be a multiple of 64 between 2048 and 4096 (got {bits})"));
    }
    let rng = ring::rand::SystemRandom::new();
    let e = BigUint::from(65537u32);
    let half = bits / 2;
    let (p, q) = loop {
        let p = random_prime(&rng, half, &e)?;
        let q = random_prime(&rng, half, &e)?;
        if p == q {
            continue;
        }
        let n = &p * &q;
        if n.bits() as u32 != bits {
            continue;
        }
        break if p > q { (p, q) } else { (q, p) };
    };
    let n = &p * &q;
    let one = BigUint::one();
    let p1 = &p - &one;
    let q1 = &q - &one;
    let lambda = p1.lcm(&q1);
    let d = e.modinv(&lambda).ok_or("rsa_generate: e not invertible")?;
    let dp = &d % &p1;
    let dq = &d % &q1;
    let qinv = q.modinv(&p).ok_or("rsa_generate: q not invertible mod p")?;
    debug_assert!(!d.is_zero());
    // RSAPrivateKey ::= SEQUENCE { version 0, n, e, d, p, q, dP, dQ, qInv }
    let mut body = der_int(&BigUint::zero());
    for v in [&n, &e, &d, &p, &q, &dp, &dq, &qinv] {
        body.extend(der_int(v));
    }
    let rsa_private_key = der_tlv(0x30, &body);
    // PrivateKeyInfo ::= SEQUENCE { version 0, AlgorithmIdentifier { rsaEncryption, NULL }, OCTET STRING }
    let alg = der_tlv(0x30, &[&der_tlv(0x06, &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01])[..], &[0x05, 0x00][..]].concat());
    let mut info = der_int(&BigUint::zero());
    info.extend(alg);
    info.extend(der_tlv(0x04, &rsa_private_key));
    Ok(der_tlv(0x30, &info))
}

#[cfg(all(feature = "crypto", feature = "bigint"))]
const MR_ROUNDS: usize = 64;

#[cfg(all(feature = "crypto", feature = "bigint"))]
const SMALL_PRIMES: [u32; 53] = [
    3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89, 97, 101, 103, 107, 109,
    113, 127, 131, 137, 139, 149, 151, 157, 163, 167, 173, 179, 181, 191, 193, 197, 199, 211, 223, 227, 229, 233,
    239, 241, 251,
];

/// Un primo aleatorio de exactamente `bits` bits (dos bits altos a 1) coprimo con `e`.
#[cfg(all(feature = "crypto", feature = "bigint"))]
fn random_prime(rng: &ring::rand::SystemRandom, bits: u32, e: &num_bigint::BigUint) -> Result<num_bigint::BigUint, String> {
    use num_bigint::BigUint;
    use num_integer::Integer;
    use num_traits::One;
    use ring::rand::SecureRandom;
    let nbytes = (bits as usize).div_ceil(8);
    let mut buf = vec![0u8; nbytes];
    'outer: loop {
        rng.fill(&mut buf).map_err(|_| "rsa_generate: the system CSPRNG failed".to_string())?;
        let mut cand = BigUint::from_bytes_be(&buf);
        // Exactamente `bits` bits, los dos altos a 1, impar.
        let mask = (BigUint::one() << bits) - BigUint::one();
        cand &= mask;
        cand |= BigUint::one() << (bits - 1);
        cand |= BigUint::one() << (bits - 2);
        cand |= BigUint::one();
        for sp in SMALL_PRIMES {
            if (&cand % sp).is_zero_u32() {
                continue 'outer;
            }
        }
        if !(&cand - BigUint::one()).gcd(e).is_one() {
            continue;
        }
        if miller_rabin(rng, &cand, MR_ROUNDS)? {
            return Ok(cand);
        }
    }
}

#[cfg(all(feature = "crypto", feature = "bigint"))]
trait IsZeroU32 {
    fn is_zero_u32(&self) -> bool;
}
#[cfg(all(feature = "crypto", feature = "bigint"))]
impl IsZeroU32 for num_bigint::BigUint {
    fn is_zero_u32(&self) -> bool {
        num_traits::Zero::is_zero(self)
    }
}

/// Miller-Rabin con bases aleatorias del CSPRNG. `n` impar > 3.
#[cfg(all(feature = "crypto", feature = "bigint"))]
fn miller_rabin(rng: &ring::rand::SystemRandom, n: &num_bigint::BigUint, rounds: usize) -> Result<bool, String> {
    use num_bigint::BigUint;
    use num_traits::{One, Zero};
    use ring::rand::SecureRandom;
    let one = BigUint::one();
    let two = BigUint::from(2u32);
    let n1 = n - &one;
    // n - 1 = 2^s · d
    let s = n1.trailing_zeros().unwrap_or(0);
    let d = &n1 >> s;
    let nbytes = (n.bits() as usize).div_ceil(8);
    let mut buf = vec![0u8; nbytes];
    'rounds: for _ in 0..rounds {
        // Base a en [2, n-2].
        let a = loop {
            rng.fill(&mut buf).map_err(|_| "rsa_generate: the system CSPRNG failed".to_string())?;
            let a = BigUint::from_bytes_be(&buf) % &n1;
            if a >= two {
                break a;
            }
        };
        let mut x = a.modpow(&d, n);
        if x == one || x == n1 {
            continue;
        }
        let mut i = BigUint::one();
        let s_big = BigUint::from(s);
        while i < s_big {
            x = x.modpow(&two, n);
            if x == n1 {
                continue 'rounds;
            }
            if x.is_zero() || x == one {
                return Ok(false);
            }
            i += &one;
        }
        return Ok(false);
    }
    Ok(true)
}

/// INTEGER DER (sin signo → 0x00 delante si el bit alto está puesto).
#[cfg(all(feature = "crypto", feature = "bigint"))]
fn der_int(v: &num_bigint::BigUint) -> Vec<u8> {
    let mut mag = v.to_bytes_be();
    if mag.first().is_some_and(|b| b & 0x80 != 0) {
        mag.insert(0, 0);
    }
    der_tlv(0x02, &mag)
}

#[cfg(all(feature = "crypto", feature = "bigint"))]
fn der_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 128 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len() - 1);
        out.push(0x80 | (bytes.len() - first) as u8);
        out.extend_from_slice(&bytes[first..]);
    }
    out.extend_from_slice(content);
    out
}

#[cfg(not(all(feature = "crypto", feature = "bigint")))]
pub fn generate_pkcs8(_bits: u32) -> Result<Vec<u8>, String> {
    Err("rsa_generate needs the 'crypto' and 'bigint' features of this build".to_string())
}
