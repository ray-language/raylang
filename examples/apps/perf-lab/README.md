# perf-lab

The same report over an access log computed three ways (a first version, an optimized one and a
parallel one), to measure. It is the project of the handbook chapter
[Performance](../../../handbook/performance.en.md).

```sh
ray test                            # the three versions agree
ray run -- slow 200000              # the first version, on the VM
ray profile -- slow 200000          # where the time goes
ray build --native --release -o perf-lab && ./perf-lab parallel 2000000
```
