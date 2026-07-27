function main() {
    const _t0 = process.hrtime.bigint();
    const n = 1000000;
    let seed = 12345;
    const arr = new Array(n);
    for (let i = 0; i < n; i++) {
        seed = (48271 * seed) % 2147483647;
        arr[i] = seed % 1000000;
    }

    arr.sort((a, b) => a - b);

    let checksum = 0;
    for (let i = 0; i < n; i++) {
        checksum = (checksum * 31 + arr[i]) % 1000000007;
    }

    console.log(arr[0]);
    console.log(arr[n - 1]);
    console.log(checksum);
    console.error(`bench_ns=${process.hrtime.bigint() - _t0}`);
}

main();
