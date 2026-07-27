use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);

sub main {
  my $_t0 = clock_gettime(CLOCK_MONOTONIC);
  my $acc = 0;
  for my $i (1 .. 10000000) {
    $acc = ($acc + $i * $i) % 1000000007;
  }
  print $acc . "\n";
  printf STDERR "bench_ns=%.0f\n", (clock_gettime(CLOCK_MONOTONIC) - $_t0) * 1e9;
}

main();
