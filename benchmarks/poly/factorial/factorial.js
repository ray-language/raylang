function fact(n) {
  if (n < 2) return 1;
  return n * fact(n - 1);
}

function main() {
  const _t0 = process.hrtime.bigint();
  for (let i = 0; i < 10; i++) {
    console.log(fact(i));
  }
  console.error(`bench_ns=${process.hrtime.bigint() - _t0}`);
}

main();
