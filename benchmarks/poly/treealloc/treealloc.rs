struct Node {
    left: Option<Box<Node>>,
    right: Option<Box<Node>>,
}

fn make_tree(depth: i64) -> Option<Box<Node>> {
    if depth == 0 {
        return None;
    }
    Some(Box::new(Node {
        left: make_tree(depth - 1),
        right: make_tree(depth - 1),
    }))
}

fn node_count(n: &Option<Box<Node>>) -> i64 {
    match n {
        None => 0,
        Some(node) => 1 + node_count(&node.left) + node_count(&node.right),
    }
}

fn main() {
    let t0 = std::time::Instant::now();
    let min_depth = 4;
    let max_depth = 14;

    let stretch = make_tree(max_depth + 1);
    println!("{}", node_count(&stretch));

    let long_lived = make_tree(max_depth);

    let mut total_check: i64 = 0;
    let mut depth = min_depth;
    while depth <= max_depth {
        let iterations = 1i64 << (max_depth - depth + min_depth);
        let mut check: i64 = 0;
        for _ in 0..iterations {
            check += node_count(&make_tree(depth));
        }
        total_check += check;
        depth += 2;
    }

    println!("{}", node_count(&long_lived));
    println!("{}", total_check);
    eprintln!("bench_ns={}", t0.elapsed().as_nanos());
}
