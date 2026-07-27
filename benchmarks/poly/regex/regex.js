function main() {
    const _t0 = process.hrtime.bigint();
    const n = 200000;
    const pattern = /^user(\d+) GET \/api\/(\d+) (\d+) (\d+)ms$/;
    let checksum = 0;
    let matchCount = 0;

    for (let i = 0; i < n; i++) {
        const status = i % 5 !== 4 ? 200 : 404;
        const line = `user${i} GET /api/${i % 50} ${status} ${i % 250}ms`;
        const m = pattern.exec(line);
        if (m) {
            matchCount++;
            const uid = parseInt(m[1], 10);
            const path = parseInt(m[2], 10);
            const st = parseInt(m[3], 10);
            const ms = parseInt(m[4], 10);
            checksum = (checksum * 31 + uid + path + st + ms) % 1000000007;
        }
    }

    console.log(matchCount);
    console.log(checksum);
    console.error(`bench_ns=${process.hrtime.bigint() - _t0}`);
}

main();
