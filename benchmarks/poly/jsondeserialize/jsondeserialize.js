function main() {
    const _t0 = process.hrtime.bigint();
    const n = 400000;
    let checksum = 0;
    let totalNameLen = 0;

    for (let i = 0; i < n; i++) {
        const line = '{"id":' + i + ',"name":"user' + i + '","score":' + (i % 100) + "}";

        const idStart = line.indexOf(":") + 1;
        const idEnd = line.indexOf(",");
        const idVal = parseInt(line.slice(idStart, idEnd), 10);

        const namePrefix = '"name":"';
        const nameStart = line.indexOf(namePrefix) + namePrefix.length;
        const nameEnd = line.indexOf('"', nameStart);
        const nameVal = line.slice(nameStart, nameEnd);

        const scoreStart = line.lastIndexOf(":") + 1;
        const scoreEnd = line.lastIndexOf("}");
        const scoreVal = parseInt(line.slice(scoreStart, scoreEnd), 10);

        checksum = (checksum * 31 + idVal + scoreVal) % 1000000007;
        totalNameLen += nameVal.length;
    }

    console.log(checksum);
    console.log(totalNameLen);
    console.error(`bench_ns=${process.hrtime.bigint() - _t0}`);
}

main();
