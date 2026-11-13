
const t0 = process.hrtime.bigint();
const rows = []; let seed = 42;
for (let i = 0; i < 300000; i++) { seed = (seed * 1103515245 + 12345) % 2147483648; rows.push({ id: i, score: seed % 100000, name: "n" + (seed % 1000) }); }
const byScore = rows.slice().sort((a, b) => a.score - b.score || a.id - b.id);
const byName = rows.slice().sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : a.id - b.id));
console.log(byScore[0].id + byScore[299999].id); console.log(byName[0].id + byName[299999].id);
process.stderr.write(`bench_ns=${process.hrtime.bigint() - t0}\n`);
