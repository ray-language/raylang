class Sq { constructor(s) { this.s = s; } area() { return this.s * this.s; } }
class Rc { constructor(w, h) { this.w = w; this.h = h; } area() { return this.w * this.h; } }
function total(xs) { let t = 0; for (const x of xs) t = (t + x.area()) % 1000003; return t; }
const t0 = process.hrtime.bigint();
const sq = [], rc = [], mixed = [];
for (let i = 0; i < 10000; i++) { sq.push(new Sq(i % 100)); rc.push(new Rc(i % 50, i % 30)); mixed.push(i % 2 === 0 ? new Sq(i % 100) : new Rc(i % 50, i % 30)); }
let acc = 0;
for (let r = 0; r < 500; r++) acc = (acc + total(sq) + total(rc) + total(mixed)) % 1000003;
console.log(acc);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
