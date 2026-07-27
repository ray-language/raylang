<?php

function make_tree($depth) {
    if ($depth === 0) {
        return null;
    }
    return ["left" => make_tree($depth - 1), "right" => make_tree($depth - 1)];
}

function node_count($n) {
    if ($n === null) {
        return 0;
    }
    return 1 + node_count($n["left"]) + node_count($n["right"]);
}

function main() {
    $_t0 = hrtime(true);
    $min_depth = 4;
    $max_depth = 14;

    $stretch = make_tree($max_depth + 1);
    echo node_count($stretch) . "\n";

    $long_lived = make_tree($max_depth);

    $total_check = 0;
    for ($depth = $min_depth; $depth <= $max_depth; $depth += 2) {
        $iterations = 1 << ($max_depth - $depth + $min_depth);
        $check = 0;
        for ($i = 0; $i < $iterations; $i++) {
            $check += node_count(make_tree($depth));
        }
        $total_check += $check;
    }

    echo node_count($long_lived) . "\n";
    echo $total_check . "\n";
    fwrite(STDERR, "bench_ns=" . (hrtime(true) - $_t0) . "\n");
}

main();
