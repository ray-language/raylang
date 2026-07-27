use std::collections::HashMap;

fn main() {
    let t0 = std::time::Instant::now();
    let base = "the quick brown fox jumps over the lazy dog and runs away fast today";
    let mut m: HashMap<String, i64> = HashMap::new();
    for r in 0..120_000 {
        let line = format!("{} {}", r % 1000, base);
        for w in line.split(' ') {
            *m.entry(w.to_string()).or_insert(0) += 1;
        }
    }
    let mut keys: Vec<&String> = m.keys().collect();
    keys.sort();
    let mut acc: i64 = 0;
    for k in keys {
        acc = (acc * 31 + m[k]) % 1_000_000_007;
    }
    println!("{}", m.len());
    println!("{}", acc);
    eprintln!("bench_ns={}", t0.elapsed().as_nanos());
}
