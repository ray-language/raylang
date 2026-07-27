const _t0 = process.hrtime.bigint();
const statuses = ["200", "200", "200", "404", "500"];
const cnt = new Map();
const lat = new Map();
for (let i = 0; i < 150000; i++) {
  const path = "/api/" + String(i % 50);
  const status = statuses[i % 5];
  const line = "GET " + path + " " + status + " " + String(i % 250);
  const f = line.split(" ");
  cnt.set(f[2], (cnt.get(f[2]) || 0) + 1);
  lat.set(f[1], (lat.get(f[1]) || 0) + parseInt(f[3], 10));
}
for (const k of [...cnt.keys()].sort()) {
  console.log(k + " " + String(cnt.get(k)));
}
let total = 0;
for (const v of lat.values()) total += v;
console.log(total);
console.error(`bench_ns=${process.hrtime.bigint() - _t0}`);
