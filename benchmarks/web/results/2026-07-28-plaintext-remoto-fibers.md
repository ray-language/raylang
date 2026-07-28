## Entorno

- **fecha**: 2026-07-28T17:36:23-04:00
- **cpu**: Apple M3 Pro
- **os**: macOS-26.5.2-arm64-arm-64bit-Mach-O
- **ray**: raylang 1.0.0
- **go**: go version go1.23.1 darwin/arm64
- **rustc**: rustc 1.96.0 (ac68faa20 2026-05-25)
- **node**: v26.3.1
- **oha**: oha 1.15.0
- **escalón**: plaintext
- **carga**: -c 100, 10s × 3 rep/escalón, SLO p99 <= 10.0 ms
- **fds**: 1048576
- **latency-correction**: no (default)
- **generador**: remoto vía SSH: roberto@10.0.0.100 → 10.0.0.11

## plaintext — veredicto

_Dos implementaciones comparten puesto (marcado con '=') si sostienen la misma tasa Y sus ventanas de p99 (mediana ± 2·MAD) se solapan: con esos datos no están separadas._

| Implementación | Tasa sostenida bajo SLO | p50 | p99 | p99 MAD | p99.9 | Primer escalón fallido |
|---|---|---|---|---|---|---|
| hyper | 200,000 rps (1.00x líder) | 0.45 ms | 0.73 ms | ±0.00 | 0.96 ms | 240,000 rps (techo: solo 205,974) |
| ray-fib | 160,000 rps (0.80x líder) | 0.47 ms | 0.73 ms | ±0.00 | 1.05 ms | 200,000 rps (techo: solo 190,553) |
| ray | 160,000 rps (0.80x líder) | 0.59 ms | 0.73 ms | ±0.00 | 1.17 ms | 200,000 rps (techo: solo 163,975) |
| go | 120,000 rps (0.60x líder) | 0.77 ms | 2.16 ms | ±0.00 | 2.71 ms | 160,000 rps (techo: solo 121,554) |
| node | 40,000 rps (0.20x líder) | 0.72 ms | 1.73 ms | ±0.04 | 2.68 ms | 80,000 rps (techo: solo 60,087) |

