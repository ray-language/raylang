use strict;
use warnings;
use Time::HiRes qw(clock_gettime CLOCK_MONOTONIC);

sub make_tree {
    my ($depth) = @_;
    return undef if $depth == 0;
    return { left => make_tree($depth - 1), right => make_tree($depth - 1) };
}

sub node_count {
    my ($n) = @_;
    return 0 unless defined $n;
    return 1 + node_count($n->{left}) + node_count($n->{right});
}

sub main {
    my $_t0 = clock_gettime(CLOCK_MONOTONIC);
    my $min_depth = 4;
    my $max_depth = 14;

    my $stretch = make_tree($max_depth + 1);
    print node_count($stretch), "\n";

    my $long_lived = make_tree($max_depth);

    my $total_check = 0;
    for (my $depth = $min_depth; $depth <= $max_depth; $depth += 2) {
        my $iterations = 1 << ($max_depth - $depth + $min_depth);
        my $check = 0;
        for (my $i = 0; $i < $iterations; $i++) {
            $check += node_count(make_tree($depth));
        }
        $total_check += $check;
    }

    print node_count($long_lived), "\n";
    print "$total_check\n";
    printf STDERR "bench_ns=%.0f\n", (clock_gettime(CLOCK_MONOTONIC) - $_t0) * 1e9;
}

main();
