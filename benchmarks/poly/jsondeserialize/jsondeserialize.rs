fn main() {
    let t0 = std::time::Instant::now();
    let n = 400000;
    let mut checksum: i64 = 0;
    let mut total_name_len: i64 = 0;

    for i in 0..n {
        let line = format!(r#"{{"id":{},"name":"user{}","score":{}}}"#, i, i, i % 100);

        let id_start = line.find(':').unwrap() + 1;
        let id_end = line.find(',').unwrap();
        let id_val: i64 = line[id_start..id_end].parse().unwrap();

        let name_prefix = r#""name":""#;
        let name_start = line.find(name_prefix).unwrap() + name_prefix.len();
        let name_end = name_start + line[name_start..].find('"').unwrap();
        let name_val = &line[name_start..name_end];

        let score_start = line.rfind(':').unwrap() + 1;
        let score_end = line.rfind('}').unwrap();
        let score_val: i64 = line[score_start..score_end].parse().unwrap();

        checksum = (checksum * 31 + id_val + score_val) % 1000000007;
        total_name_len += name_val.len() as i64;
    }

    println!("{}", checksum);
    println!("{}", total_name_len);
    eprintln!("bench_ns={}", t0.elapsed().as_nanos());
}
