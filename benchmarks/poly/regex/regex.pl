use strict;
use warnings;
use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);

sub main {
    my $_t0 = clock_gettime(CLOCK_MONOTONIC);
    my $n = 200000;
    my $checksum = 0;
    my $match_count = 0;

    for (my $i = 0; $i < $n; $i++) {
        my $status = ($i % 5 != 4) ? 200 : 404;
        my $line = "user$i GET /api/" . ($i % 50) . " $status " . ($i % 250) . "ms";

        if ($line =~ /^user(\d+) GET \/api\/(\d+) (\d+) (\d+)ms$/) {
            $match_count++;
            $checksum = ($checksum * 31 + $1 + $2 + $3 + $4) % 1000000007;
        }
    }

    print "$match_count\n";
    print "$checksum\n";
    printf STDERR "bench_ns=%.0f\n", (clock_gettime(CLOCK_MONOTONIC) - $_t0) * 1e9;
}

main();
