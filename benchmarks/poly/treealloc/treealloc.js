function makeTree(depth) {
    if (depth === 0) return null;
    return { left: makeTree(depth - 1), right: makeTree(depth - 1) };
}

function nodeCount(n) {
    if (n === null) return 0;
    return 1 + nodeCount(n.left) + nodeCount(n.right);
}

function main() {
    const _t0 = process.hrtime.bigint();
    const minDepth = 4;
    const maxDepth = 14;

    const stretch = makeTree(maxDepth + 1);
    console.log(nodeCount(stretch));

    const longLived = makeTree(maxDepth);

    let totalCheck = 0;
    for (let depth = minDepth; depth <= maxDepth; depth += 2) {
        const iterations = 1 << (maxDepth - depth + minDepth);
        let check = 0;
        for (let i = 0; i < iterations; i++) {
            check += nodeCount(makeTree(depth));
        }
        totalCheck += check;
    }

    console.log(nodeCount(longLived));
    console.log(totalCheck);
    console.error(`bench_ns=${process.hrtime.bigint() - _t0}`);
}

main();
