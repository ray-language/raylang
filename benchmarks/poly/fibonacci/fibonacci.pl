use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);

sub fib {
  my ($n) = @_;
  return $n if $n < 2;
  return fib($n - 1) + fib($n - 2);
}

sub main {
  my $_t0 = clock_gettime(CLOCK_MONOTONIC);
  for my $i (0 .. 9) {
    print fib($i) . "\n";
  }
  printf STDERR "bench_ns=%.0f\n", (clock_gettime(CLOCK_MONOTONIC) - $_t0) * 1e9;
}

main();
