fn main() {
    let t0 = std::time::Instant::now();
    let n = 1_000_000i64;
    let mut seed: i64 = 12345;
    let mut arr: Vec<i64> = Vec::with_capacity(n as usize);
    for _ in 0..n {
        seed = (48271 * seed) % 2147483647;
        arr.push(seed % 1000000);
    }

    arr.sort();

    let mut checksum: i64 = 0;
    for v in &arr {
        checksum = (checksum * 31 + v) % 1000000007;
    }

    println!("{}", arr[0]);
    println!("{}", arr[(n - 1) as usize]);
    println!("{}", checksum);
    eprintln!("bench_ns={}", t0.elapsed().as_nanos());
}
