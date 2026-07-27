function main() {
  const _t0 = process.hrtime.bigint();
  let acc = 0;
  for (let i = 1; i <= 10000000; i++) {
    acc = (acc + i * i) % 1000000007;
  }
  console.log(acc);
  console.error(`bench_ns=${process.hrtime.bigint() - _t0}`);
}

main();
