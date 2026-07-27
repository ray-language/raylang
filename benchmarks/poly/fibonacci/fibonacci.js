function fib(n) {
  if (n < 2) return n;
  return fib(n - 1) + fib(n - 2);
}

function main() {
  const _t0 = process.hrtime.bigint();
  for (let i = 0; i < 10; i++) {
    console.log(fib(i));
  }
  console.error(`bench_ns=${process.hrtime.bigint() - _t0}`);
}

main();
