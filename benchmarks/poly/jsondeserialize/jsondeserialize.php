<?php

function main() {
    $_t0 = hrtime(true);
    $n = 400000;
    $checksum = 0;
    $total_name_len = 0;

    for ($i = 0; $i < $n; $i++) {
        $line = '{"id":' . $i . ',"name":"user' . $i . '","score":' . ($i % 100) . '}';

        $id_start = strpos($line, ":") + 1;
        $id_end = strpos($line, ",");
        $id_val = (int)substr($line, $id_start, $id_end - $id_start);

        $name_prefix = '"name":"';
        $name_start = strpos($line, $name_prefix) + strlen($name_prefix);
        $name_end = strpos($line, '"', $name_start);
        $name_val = substr($line, $name_start, $name_end - $name_start);

        $score_start = strrpos($line, ":") + 1;
        $score_end = strrpos($line, "}");
        $score_val = (int)substr($line, $score_start, $score_end - $score_start);

        $checksum = ($checksum * 31 + $id_val + $score_val) % 1000000007;
        $total_name_len += strlen($name_val);
    }

    echo $checksum . "\n";
    echo $total_name_len . "\n";
    fwrite(STDERR, "bench_ns=" . (hrtime(true) - $_t0) . "\n");
}

main();
