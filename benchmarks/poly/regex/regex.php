<?php

function main() {
    $_t0 = hrtime(true);
    $n = 200000;
    $pattern = '/^user(\d+) GET \/api\/(\d+) (\d+) (\d+)ms$/';
    $checksum = 0;
    $match_count = 0;

    for ($i = 0; $i < $n; $i++) {
        $status = ($i % 5 !== 4) ? 200 : 404;
        $line = "user{$i} GET /api/" . ($i % 50) . " {$status} " . ($i % 250) . "ms";

        if (preg_match($pattern, $line, $m)) {
            $match_count++;
            $uid = (int)$m[1];
            $path = (int)$m[2];
            $st = (int)$m[3];
            $ms = (int)$m[4];
            $checksum = ($checksum * 31 + $uid + $path + $st + $ms) % 1000000007;
        }
    }

    echo $match_count . "\n";
    echo $checksum . "\n";
    fwrite(STDERR, "bench_ns=" . (hrtime(true) - $_t0) . "\n");
}

main();
