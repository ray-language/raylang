
const t0 = process.hrtime.bigint();
const m = new Map();
for (let i = 0; i < 100000; i++) m.set(i, { balance: 0, n: 0 });
let seed = 12345;
for (let i = 0; i < 2000000; i++) {
  seed = (seed * 1103515245 + 12345) % 2147483648;
  const a = m.get(seed % 100000);
  if (a) { a.balance += seed % 100; a.n++; }
}
let sum = 0;
for (let i = 0; i < 100000; i++) { const a = m.get(i); if (a) sum = (sum + a.balance * 31 + a.n) % 1000000007; }
console.log(sum);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
