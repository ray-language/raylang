function solve(row, n, cols, d1, d2) {
  if (row === n) return 1;
  let count = 0;
  for (let c = 0; c < n; c++) {
    const a = row + c, b = row - c + n - 1;
    if (!cols[c] && !d1[a] && !d2[b]) { cols[c] = d1[a] = d2[b] = true; count += solve(row + 1, n, cols, d1, d2); cols[c] = d1[a] = d2[b] = false; }
  }
  return count;
}
const t0 = process.hrtime.bigint();
const n = 11;
console.log(solve(0, n, new Array(n).fill(false), new Array(2 * n).fill(false), new Array(2 * n).fill(false)));
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
