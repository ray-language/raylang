function main() {
    const _t0 = process.hrtime.bigint();
    const n = 200;
    const a = [];
    const b = [];
    for (let i = 0; i < n; i++) {
        const rowA = new Array(n);
        const rowB = new Array(n);
        for (let j = 0; j < n; j++) {
            rowA[j] = (i * n + j) % 13;
            rowB[j] = (j * n + i) % 17;
        }
        a.push(rowA);
        b.push(rowB);
    }

    const c = [];
    for (let i = 0; i < n; i++) {
        const row = new Array(n).fill(0.0);
        for (let j = 0; j < n; j++) {
            let s = 0.0;
            for (let k = 0; k < n; k++) {
                s += a[i][k] * b[k][j];
            }
            row[j] = s;
        }
        c.push(row);
    }

    let checksum = 0.0;
    for (let i = 0; i < n; i++) {
        for (let j = 0; j < n; j++) {
            checksum += c[i][j];
        }
    }

    console.log(Math.trunc(checksum));
    console.error(`bench_ns=${process.hrtime.bigint() - _t0}`);
}

main();
