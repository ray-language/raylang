<?php

function fact($n) {
    if ($n < 2) return 1;
    return $n * fact($n - 1);
}

function main() {
    $_t0 = hrtime(true);
    for ($i = 0; $i < 10; $i++) {
        echo fact($i) . "\n";
    }
    fwrite(STDERR, "bench_ns=" . (hrtime(true) - $_t0) . "\n");
}

main();
