// std no trae motor de regex (el crate `regex` requeriría Cargo + red, lo que
// rompería el build de un solo archivo con `rustc -O` usado para el resto de
// los benchmarks). Se parsea a mano el mismo patrón fijo
// `^user(\d+) GET /api/(\d+) (\d+) (\d+)ms$` que las demás variantes matchean
// con su motor de regex nativo.
fn parse_line(line: &str) -> Option<(i64, i64, i64, i64)> {
    let rest = line.strip_prefix("user")?;
    let (uid_str, rest) = rest.split_once(" GET /api/")?;
    let (path_str, rest) = rest.split_once(' ')?;
    let (st_str, rest) = rest.split_once(' ')?;
    let ms_str = rest.strip_suffix("ms")?;

    let uid: i64 = uid_str.parse().ok()?;
    let path: i64 = path_str.parse().ok()?;
    let st: i64 = st_str.parse().ok()?;
    let ms: i64 = ms_str.parse().ok()?;
    Some((uid, path, st, ms))
}

fn main() {
    let t0 = std::time::Instant::now();
    let n = 200000;
    let mut checksum: i64 = 0;
    let mut match_count = 0;

    for i in 0..n {
        let status = if i % 5 != 4 { 200 } else { 404 };
        let line = format!("user{} GET /api/{} {} {}ms", i, i % 50, status, i % 250);

        if let Some((uid, path, st, ms)) = parse_line(&line) {
            match_count += 1;
            checksum = (checksum * 31 + uid + path + st + ms) % 1000000007;
        }
    }

    println!("{}", match_count);
    println!("{}", checksum);
    eprintln!("bench_ns={}", t0.elapsed().as_nanos());
}
