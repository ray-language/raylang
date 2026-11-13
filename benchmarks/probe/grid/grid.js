
const t0 = process.hrtime.bigint();
const n = 3000000; const sieve = [];
for (let i = 0; i < n; i++) sieve.push(true);
for (let i = 2; i * i < n; i++) if (sieve[i]) for (let j = i * i; j < n; j += i) sieve[j] = false;
let primes = 0; for (let k = 2; k < n; k++) if (sieve[k]) primes++;
console.log(primes);
const w = 700; const g = [];
for (let y = 0; y < w; y++) { const row = []; for (let x = 0; x < w; x++) row.push((x * 31 + y * 17) % 10); g.push(row); }
for (let y = 1; y < w; y++) for (let x = 1; x < w; x++) { const up = g[y - 1][x], left = g[y][x - 1]; g[y][x] = (g[y][x] + (up < left ? up : left)) % 1000003; }
console.log(g[w - 1][w - 1]);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
