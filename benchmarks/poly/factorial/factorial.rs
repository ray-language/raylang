fn fact(n: i64) -> i64 {
    if n < 2 {
        return 1;
    }
    n * fact(n - 1)
}

fn main() {
    let t0 = std::time::Instant::now();
    for i in 0..10 {
        println!("{}", fact(i));
    }
    eprintln!("bench_ns={}", t0.elapsed().as_nanos());
}
