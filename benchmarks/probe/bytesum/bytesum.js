
const t0 = process.hrtime.bigint();
const data = Buffer.from("the quick brown fox jumps over the lazy dog 0123456789 ".repeat(40000));
let a = 1, b = 0;
for (let r = 0; r < 4; r++) for (let i = 0; i < data.length; i++) { a = (a + data[i]) % 65521; b = (b + a) % 65521; }
console.log(b * 65536 + a);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
