use strict; use warnings;
use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);
my $_t0 = clock_gettime(CLOCK_MONOTONIC);
my $base = "the quick brown fox jumps over the lazy dog and runs away fast today";
my %m;
for my $r (0 .. 120000 - 1) {
    my $line = ($r % 1000) . " " . $base;
    for my $w (split / /, $line) {
        $m{$w}++;
    }
}
my $acc = 0;
for my $k (sort keys %m) {
    $acc = ($acc * 31 + $m{$k}) % 1000000007;
}
print scalar(keys %m), "\n";
print $acc, "\n";
printf STDERR "bench_ns=%.0f\n", (clock_gettime(CLOCK_MONOTONIC) - $_t0) * 1e9;
