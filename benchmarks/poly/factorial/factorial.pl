use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);

sub fact {
  my ($n) = @_;
  return 1 if $n < 2;
  return $n * fact($n - 1);
}

sub main {
  my $_t0 = clock_gettime(CLOCK_MONOTONIC);
  for my $i (0 .. 9) {
    print fact($i) . "\n";
  }
  printf STDERR "bench_ns=%.0f\n", (clock_gettime(CLOCK_MONOTONIC) - $_t0) * 1e9;
}

main();
