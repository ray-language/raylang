//! M195 — enteros grandes SIN SIGNO para `std/bigint` (plan ray-remote C3): la única pieza del
//! feedback del cliente RFB que raylang puro no podía resolver (dos `modpow` de 4096 bits costaban
//! 5 s en nativo; aquí, milisegundos). Los valores viajan como `bytes` big-endian (magnitud sin
//! signo, sin ceros a la izquierda salvo el `0` = `[]`/`[0]`), así que no hace falta un tipo nuevo en
//! ningún motor: una sola primitiva `op(nombre, a, b, c)` que ambos motores y el nativo llaman.
//!
//! NO es de tiempo constante (`num-bigint` no lo es): sirve para el Diffie-Hellman o el RSA de un
//! CLIENTE, no para operar con la clave privada de un servidor expuesto a medidas de tiempo.

#[cfg(feature = "bigint")]
pub fn op(name: &str, a: &[u8], b: &[u8], c: &[u8]) -> Result<Vec<u8>, String> {
    use num_bigint::BigUint;
    use num_integer::Integer;
    use num_traits::{One, Zero};
    let a = BigUint::from_bytes_be(a);
    let b = BigUint::from_bytes_be(b);
    let c = BigUint::from_bytes_be(c);
    let r = match name {
        "add" => a + b,
        "sub" => {
            if a < b {
                return Err("bigint: subtraction would be negative".to_string());
            }
            a - b
        }
        "mul" => a * b,
        "div" => {
            if b.is_zero() {
                return Err("bigint: division by zero".to_string());
            }
            a / b
        }
        "rem" => {
            if b.is_zero() {
                return Err("bigint: division by zero".to_string());
            }
            a % b
        }
        "modpow" => {
            if c.is_zero() {
                return Err("bigint: modulus is zero".to_string());
            }
            a.modpow(&b, &c)
        }
        "modinv" => {
            // Inverso modular por el algoritmo de Euclides extendido (sobre enteros con signo).
            if b.is_zero() {
                return Err("bigint: modulus is zero".to_string());
            }
            let g = a.gcd(&b);
            if !g.is_one() {
                return Err("bigint: no modular inverse (not coprime)".to_string());
            }
            match a.modinv(&b) {
                Some(v) => v,
                None => return Err("bigint: no modular inverse (not coprime)".to_string()),
            }
        }
        "gcd" => a.gcd(&b),
        // cmp: un octeto — 0 menor, 1 igual, 2 mayor.
        "cmp" => {
            let v = match a.cmp(&b) {
                std::cmp::Ordering::Less => 0u8,
                std::cmp::Ordering::Equal => 1,
                std::cmp::Ordering::Greater => 2,
            };
            return Ok(vec![v]);
        }
        // shl/shr: la cuenta viene en `b` (cabe en u32).
        "shl" => {
            let n = u32::try_from(b).map_err(|_| "bigint: shift count out of range".to_string())?;
            a << n
        }
        "shr" => {
            let n = u32::try_from(b).map_err(|_| "bigint: shift count out of range".to_string())?;
            a >> n
        }
        other => return Err(format!("bigint: unknown operation '{}'", other)),
    };
    Ok(r.to_bytes_be())
}

#[cfg(not(feature = "bigint"))]
pub fn op(_name: &str, _a: &[u8], _b: &[u8], _c: &[u8]) -> Result<Vec<u8>, String> {
    Err("bigint is not available in this build (feature 'bigint')".to_string())
}
