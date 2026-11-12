
const t0 = process.hrtime.bigint();
let count = 0; const n = 500;
for (let py = 0; py < n; py++) for (let px = 0; px < n; px++) {
  const cr = px * 3.0 / n - 2.0, ci = py * 2.0 / n - 1.0;
  let zr = 0.0, zi = 0.0, it = 0;
  while (it < 200 && zr * zr + zi * zi < 4.0) { const t = zr * zr - zi * zi + cr; zi = 2.0 * zr * zi + ci; zr = t; it++; }
  count += it;
}
console.log(count);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
