<?php
$_t0 = hrtime(true);
$parts = [];
for ($i = 0; $i < 400000; $i++) {
    $parts[] = '{"id":' . $i . ',"name":"user' . $i . '","score":' . ($i % 100) . '}';
}
$out = implode("\n", $parts);
echo strlen($out) . "\n";
echo count($parts) . "\n";
fwrite(STDERR, "bench_ns=" . (hrtime(true) - $_t0) . "\n");
