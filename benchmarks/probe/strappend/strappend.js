
const t0 = process.hrtime.bigint();
let s = ""; for (let i = 0; i < 60000; i++) s = s + "ab";
console.log(s.length);
let t = ""; for (let i = 0; i < 200000; i++) { t = t + String(i % 10); if (t.length > 64) t = ""; }
console.log(t.length);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
