fn main() {
    let t0 = std::time::Instant::now();
    let n: usize = 200;
    let mut a = vec![vec![0.0f64; n]; n];
    let mut b = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            a[i][j] = ((i * n + j) % 13) as f64;
            b[i][j] = ((j * n + i) % 17) as f64;
        }
    }

    let mut c = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0f64;
            for k in 0..n {
                s += a[i][k] * b[k][j];
            }
            c[i][j] = s;
        }
    }

    let mut checksum = 0.0f64;
    for i in 0..n {
        for j in 0..n {
            checksum += c[i][j];
        }
    }

    println!("{}", checksum as i64);
    eprintln!("bench_ns={}", t0.elapsed().as_nanos());
}
