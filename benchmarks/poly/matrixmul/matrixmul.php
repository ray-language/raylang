<?php

function main() {
    $_t0 = hrtime(true);
    $n = 200;
    $a = [];
    $b = [];
    for ($i = 0; $i < $n; $i++) {
        $rowA = [];
        $rowB = [];
        for ($j = 0; $j < $n; $j++) {
            $rowA[$j] = ($i * $n + $j) % 13;
            $rowB[$j] = ($j * $n + $i) % 17;
        }
        $a[$i] = $rowA;
        $b[$i] = $rowB;
    }

    $c = [];
    for ($i = 0; $i < $n; $i++) {
        $row = array_fill(0, $n, 0.0);
        for ($j = 0; $j < $n; $j++) {
            $s = 0.0;
            for ($k = 0; $k < $n; $k++) {
                $s += $a[$i][$k] * $b[$k][$j];
            }
            $row[$j] = $s;
        }
        $c[$i] = $row;
    }

    $checksum = 0.0;
    for ($i = 0; $i < $n; $i++) {
        for ($j = 0; $j < $n; $j++) {
            $checksum += $c[$i][$j];
        }
    }

    echo intval($checksum) . "\n";
    fwrite(STDERR, "bench_ns=" . (hrtime(true) - $_t0) . "\n");
}

main();
