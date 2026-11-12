
const t0 = process.hrtime.bigint();
const xs = []; for (let i = 0; i < 100000; i++) xs.push(i);
let acc = 0;
for (let r = 0; r < 100; r++) {
  const s = xs.map(x => x * 3 + r).filter(x => x % 2 === 0).reduce((a, x) => (a + x) % 1000003, 0);
  acc = (acc + s) % 1000003;
}
console.log(acc);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
