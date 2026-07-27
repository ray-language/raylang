fn main() {
    let t0 = std::time::Instant::now();
    let mut acc: i64 = 0;
    for i in 1..=10_000_000i64 {
        acc = (acc + i * i) % 1_000_000_007;
    }
    println!("{}", acc);
    eprintln!("bench_ns={}", t0.elapsed().as_nanos());
}
