use strict;
use warnings;
use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);

sub main {
    my $_t0 = clock_gettime(CLOCK_MONOTONIC);
    my $n = 400000;
    my $checksum = 0;
    my $total_name_len = 0;

    for (my $i = 0; $i < $n; $i++) {
        my $score = $i % 100;
        my $line = qq({"id":$i,"name":"user$i","score":$score});

        my ($id_val, $name_val, $score_val) =
            $line =~ /"id":(\d+),"name":"(user\d+)","score":(\d+)}/;

        $checksum = ($checksum * 31 + $id_val + $score_val) % 1000000007;
        $total_name_len += length($name_val);
    }

    print "$checksum\n";
    print "$total_name_len\n";
    printf STDERR "bench_ns=%.0f\n", (clock_gettime(CLOCK_MONOTONIC) - $_t0) * 1e9;
}

main();
