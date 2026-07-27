<?php

function main() {
    $_t0 = hrtime(true);
    $n = 1000000;
    $seed = 12345;
    $arr = [];
    for ($i = 0; $i < $n; $i++) {
        $seed = (48271 * $seed) % 2147483647;
        $arr[] = $seed % 1000000;
    }

    sort($arr);

    $checksum = 0;
    foreach ($arr as $v) {
        $checksum = ($checksum * 31 + $v) % 1000000007;
    }

    echo $arr[0] . "\n";
    echo $arr[$n - 1] . "\n";
    echo $checksum . "\n";
    fwrite(STDERR, "bench_ns=" . (hrtime(true) - $_t0) . "\n");
}

main();
