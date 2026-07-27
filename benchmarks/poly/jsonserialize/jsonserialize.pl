use strict; use warnings;
use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);
my $_t0 = clock_gettime(CLOCK_MONOTONIC);
my @parts;
for my $i (0 .. 400000 - 1) {
    push @parts, '{"id":' . $i . ',"name":"user' . $i . '","score":' . ($i % 100) . '}';
}
my $out = join("\n", @parts);
print length($out), "\n";
print scalar(@parts), "\n";
printf STDERR "bench_ns=%.0f\n", (clock_gettime(CLOCK_MONOTONIC) - $_t0) * 1e9;
