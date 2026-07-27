fn fib(n: i64) -> i64 {
    if n < 2 {
        return n;
    }
    fib(n - 1) + fib(n - 2)
}

fn main() {
    let t0 = std::time::Instant::now();
    println!("{}", fib(34));
    eprintln!("bench_ns={}", t0.elapsed().as_nanos());
}
