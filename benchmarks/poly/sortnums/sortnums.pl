use strict;
use warnings;
use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);

sub main {
    my $_t0 = clock_gettime(CLOCK_MONOTONIC);
    my $n = 1000000;
    my $seed = 12345;
    my @arr;
    for (my $i = 0; $i < $n; $i++) {
        $seed = (48271 * $seed) % 2147483647;
        push @arr, $seed % 1000000;
    }

    @arr = sort { $a <=> $b } @arr;

    my $checksum = 0;
    for my $v (@arr) {
        $checksum = ($checksum * 31 + $v) % 1000000007;
    }

    print "$arr[0]\n";
    print "$arr[$n - 1]\n";
    print "$checksum\n";
    printf STDERR "bench_ns=%.0f\n", (clock_gettime(CLOCK_MONOTONIC) - $_t0) * 1e9;
}

main();
