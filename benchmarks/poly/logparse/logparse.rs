use std::collections::HashMap;

fn main() {
    let t0 = std::time::Instant::now();
    let statuses = ["200", "200", "200", "404", "500"];
    let mut cnt: HashMap<String, i64> = HashMap::new();
    let mut lat: HashMap<String, i64> = HashMap::new();
    for i in 0..150_000 {
        let path = format!("/api/{}", i % 50);
        let status = statuses[i % 5];
        let line = format!("GET {} {} {}", path, status, i % 250);
        let f: Vec<&str> = line.split(' ').collect();
        *cnt.entry(f[2].to_string()).or_insert(0) += 1;
        let n: i64 = f[3].parse().unwrap_or(0);
        *lat.entry(f[1].to_string()).or_insert(0) += n;
    }
    let mut keys: Vec<&String> = cnt.keys().collect();
    keys.sort();
    for k in keys {
        println!("{} {}", k, cnt[k]);
    }
    let total: i64 = lat.values().sum();
    println!("{}", total);
    eprintln!("bench_ns={}", t0.elapsed().as_nanos());
}
