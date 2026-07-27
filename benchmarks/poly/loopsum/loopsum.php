<?php

function main() {
    $_t0 = hrtime(true);
    $acc = 0;
    for ($i = 1; $i <= 10000000; $i++) {
        $acc = ($acc + $i * $i) % 1000000007;
    }
    echo $acc . "\n";
    fwrite(STDERR, "bench_ns=" . (hrtime(true) - $_t0) . "\n");
}

main();
