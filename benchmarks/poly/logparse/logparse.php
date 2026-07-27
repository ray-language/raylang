<?php
$_t0 = hrtime(true);
$statuses = ["200", "200", "200", "404", "500"];
$cnt = [];
$lat = [];
for ($i = 0; $i < 150000; $i++) {
    $path = "/api/" . ($i % 50);
    $status = $statuses[$i % 5];
    $line = "GET " . $path . " " . $status . " " . ($i % 250);
    $f = explode(" ", $line);
    $cnt[$f[2]] = ($cnt[$f[2]] ?? 0) + 1;
    $lat[$f[1]] = ($lat[$f[1]] ?? 0) + intval($f[3]);
}
$keys = array_keys($cnt);
sort($keys, SORT_STRING);
foreach ($keys as $k) {
    echo $k . " " . $cnt[(string)$k] . "\n";
}
echo array_sum($lat) . "\n";
fwrite(STDERR, "bench_ns=" . (hrtime(true) - $_t0) . "\n");
