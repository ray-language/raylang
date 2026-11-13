
const t0 = process.hrtime.bigint();
const ps = [];
for (let i = 0; i < 1000; i++) ps.push({ x: i, y: i * 0.5, vx: 1.0 + i * 0.001, vy: 0.5 - i * 0.0005 });
for (let step = 0; step < 3000; step++) {
  for (const p of ps) {
    p.x = p.x + p.vx * 0.01; p.y = p.y + p.vy * 0.01;
    if (p.x > 1000.0) p.vx = 0.0 - p.vx;
    if (p.y < 0.0) p.vy = 0.0 - p.vy;
  }
}
let sum = 0; for (const p of ps) sum += p.x + p.y;
console.log(Math.trunc(sum));
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
