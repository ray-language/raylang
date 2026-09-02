---
name: Bug report
about: Something behaves wrong (a crash, a wrong result, an engine divergence)
title: ''
labels: bug
assignees: ''
---

## Minimal program

<!-- The single most useful thing: the SMALLEST .ray program that shows the problem. -->

```rust
fn main() {
    // ...
}
```

## Expected vs. observed

<!-- What you expected, what actually happened. Paste the exact output/diagnostic. -->

## Engines

<!-- raylang has three engines that must be byte-identical. Which did you try?
     `ray run prog.ray` (VM, default) · `ray run --interp prog.ray` (interpreter) ·
     `ray build --native prog.ray -o prog && ./prog` (native).
     If any two DISAGREE, say so — divergence is always a bug, and the highest-priority kind. -->

- [ ] VM
- [ ] Interpreter
- [ ] Native binary
- [ ] They disagree with each other (which ones?)

## Environment

- `ray version` output:
- OS:
