<?php

function fib($n) {
    if ($n < 2) return $n;
    return fib($n - 1) + fib($n - 2);
}

function main() {
    $_t0 = hrtime(true);
    for ($i = 0; $i < 10; $i++) {
        echo fib($i) . "\n";
    }
    fwrite(STDERR, "bench_ns=" . (hrtime(true) - $_t0) . "\n");
}

main();
