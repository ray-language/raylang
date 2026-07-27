const _t0 = process.hrtime.bigint();
const parts = [];
for (let i = 0; i < 400000; i++) {
  parts.push('{"id":' + String(i) + ',"name":"user' + String(i) + '","score":' + String(i % 100) + '}');
}
const out = parts.join("\n");
console.log(out.length);
console.log(parts.length);
console.error(`bench_ns=${process.hrtime.bigint() - _t0}`);
