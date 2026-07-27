use strict;
use warnings;
use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);

sub main {
    my $_t0 = clock_gettime(CLOCK_MONOTONIC);
    my $n = 200;
    my (@a, @b, @c);
    for (my $i = 0; $i < $n; $i++) {
        for (my $j = 0; $j < $n; $j++) {
            $a[$i][$j] = ($i * $n + $j) % 13;
            $b[$i][$j] = ($j * $n + $i) % 17;
            $c[$i][$j] = 0.0;
        }
    }

    for (my $i = 0; $i < $n; $i++) {
        for (my $j = 0; $j < $n; $j++) {
            my $s = 0.0;
            for (my $k = 0; $k < $n; $k++) {
                $s += $a[$i][$k] * $b[$k][$j];
            }
            $c[$i][$j] = $s;
        }
    }

    my $checksum = 0.0;
    for (my $i = 0; $i < $n; $i++) {
        for (my $j = 0; $j < $n; $j++) {
            $checksum += $c[$i][$j];
        }
    }

    print int($checksum), "\n";
    printf STDERR "bench_ns=%.0f\n", (clock_gettime(CLOCK_MONOTONIC) - $_t0) * 1e9;
}

main();
