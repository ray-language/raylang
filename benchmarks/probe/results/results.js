function step(x) { return x % 97 === 0 ? { ok: false, e: "bad" } : { ok: true, v: x * 2 % 1000003 }; }
function run(x) { const a = step(x); if (!a.ok) return a; const b = step(a.v + 1); if (!b.ok) return b; return { ok: true, v: a.v + b.v }; }
const t0 = process.hrtime.bigint();
let ok = 0, sum = 0;
for (let i = 0; i < 5000000; i++) { const r = run(i); if (r.ok) { ok++; sum = (sum + r.v) % 1000003; } }
console.log(ok); console.log(sum);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
