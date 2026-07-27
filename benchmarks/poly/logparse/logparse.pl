use strict; use warnings;
use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);
my $_t0 = clock_gettime(CLOCK_MONOTONIC);
my @statuses = ("200", "200", "200", "404", "500");
my (%cnt, %lat);
for my $i (0 .. 150000 - 1) {
    my $path = "/api/" . ($i % 50);
    my $status = $statuses[$i % 5];
    my $line = "GET " . $path . " " . $status . " " . ($i % 250);
    my @f = split / /, $line;
    $cnt{$f[2]}++;
    $lat{$f[1]} += int($f[3]);
}
for my $k (sort keys %cnt) {
    print $k . " " . $cnt{$k} . "\n";
}
my $total = 0;
$total += $_ for values %lat;
print $total, "\n";
printf STDERR "bench_ns=%.0f\n", (clock_gettime(CLOCK_MONOTONIC) - $_t0) * 1e9;
