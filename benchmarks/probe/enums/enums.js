
const t0 = process.hrtime.bigint();
const prog = [];
for (let i = 0; i < 200; i++) prog.push({k:0,v:i+1},{k:1},{k:3},{k:2},{k:5,v:1000003});
let total = 0;
for (let r = 0; r < 3000; r++) {
  const stack = [r];
  for (const op of prog) {
    switch (op.k) {
      case 0: stack.push(op.v); break;
      case 1: { const b = stack.pop(), a = stack.pop(); stack.push(a + b); break; }
      case 2: { const b = stack.pop(), a = stack.pop(); stack.push(a * b); break; }
      case 3: stack.push(stack[stack.length - 1]); break;
      case 4: { const b = stack.pop(), a = stack.pop(); stack.push(b); stack.push(a); break; }
      case 5: { const a = stack.pop(); stack.push(a % op.v); break; }
    }
  }
  total = (total + stack[0]) % 1000003;
}
console.log(total);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
