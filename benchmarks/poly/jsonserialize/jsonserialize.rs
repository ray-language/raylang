fn main() {
    let t0 = std::time::Instant::now();
    let mut parts: Vec<String> = Vec::with_capacity(400_000);
    for i in 0..400_000 {
        parts.push(format!(
            "{{\"id\":{},\"name\":\"user{}\",\"score\":{}}}",
            i,
            i,
            i % 100
        ));
    }
    let out = parts.join("\n");
    println!("{}", out.len());
    println!("{}", parts.len());
    eprintln!("bench_ns={}", t0.elapsed().as_nanos());
}
