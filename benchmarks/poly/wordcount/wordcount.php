<?php
$_t0 = hrtime(true);
$base = "the quick brown fox jumps over the lazy dog and runs away fast today";
$m = [];
for ($r = 0; $r < 120000; $r++) {
    $line = ($r % 1000) . " " . $base;
    foreach (explode(" ", $line) as $w) {
        $m[$w] = ($m[$w] ?? 0) + 1;
    }
}
$keys = array_keys($m);
sort($keys, SORT_STRING);
$acc = 0;
foreach ($keys as $k) {
    $acc = ($acc * 31 + $m[(string)$k]) % 1000000007;
}
echo count($m) . "\n";
echo $acc . "\n";
fwrite(STDERR, "bench_ns=" . (hrtime(true) - $_t0) . "\n");
