const _t0 = process.hrtime.bigint();
const base = "the quick brown fox jumps over the lazy dog and runs away fast today";
const m = new Map();
for (let r = 0; r < 120000; r++) {
  const line = String(r % 1000) + " " + base;
  for (const w of line.split(" ")) {
    m.set(w, (m.get(w) || 0) + 1);
  }
}
let acc = 0;
for (const k of [...m.keys()].sort()) {
  acc = (acc * 31 + m.get(k)) % 1000000007;
}
console.log(m.size);
console.log(acc);
console.error(`bench_ns=${process.hrtime.bigint() - _t0}`);
