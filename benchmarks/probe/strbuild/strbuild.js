
const t0 = process.hrtime.bigint();
let total = 0; const parts = [];
for (let i = 0; i < 300000; i++) {
  const s = `id=${i} name=user${i % 1000} ok=${i % 2 === 0}`;
  total += s.length;
  if (i % 10 === 0) parts.push(s);
}
const joined = parts.join(",");
console.log(total); console.log(joined.length);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
