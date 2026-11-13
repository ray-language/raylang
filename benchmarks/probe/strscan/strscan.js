
const t0 = process.hrtime.bigint();
const line = "GET /api/v1/users/12345?limit=50&offset=200 HTTP/1.1 host=example.com ua=Mozilla/5.0 status=200 bytes=5123";
let digits = 0, words = 0, numsum = 0;
for (let r = 0; r < 40000; r++) {
  let cur = 0, inNum = false;
  for (let i = 0; i < line.length; i++) {
    const c = line.charCodeAt(i);
    if (c >= 48 && c <= 57) { digits++; cur = cur * 10 + (c - 48); inNum = true; } else {
      if (inNum) { numsum = (numsum + cur) % 1000003; cur = 0; inNum = false; }
      if (c === 32) words++;
    }
  }
  if (inNum) numsum = (numsum + cur) % 1000003;
}
console.log(digits); console.log(words); console.log(numsum);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
