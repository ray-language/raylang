## Entorno

- **fecha**: 2026-07-28T19:33:02-04:00
- **cpu**: Apple M3 Pro
- **os**: macOS-26.5.2-arm64-arm-64bit-Mach-O
- **ray**: raylang 1.0.0
- **go**: go version go1.23.1 darwin/arm64
- **rustc**: rustc 1.96.0 (ac68faa20 2026-05-25)
- **node**: v26.3.1
- **oha**: oha 1.15.0
- **escalón**: json
- **carga**: -c 100, 10s × 3 rep/escalón, SLO p99 <= 10.0 ms
- **fds**: 1048576
- **latency-correction**: no (default)
- **generador**: remoto vía SSH: roberto@10.0.0.100 → 10.0.0.11

## json — veredicto

_Dos implementaciones comparten puesto (marcado con '=') si sostienen la misma tasa Y sus ventanas de p99 (mediana ± 2·MAD) se solapan: con esos datos no están separadas._

| Implementación | Tasa sostenida bajo SLO | p50 | p99 | p99 MAD | p99.9 | Primer escalón fallido |
|---|---|---|---|---|---|---|
| axum | 200,000 rps (1.00x líder) | 0.47 ms | 0.77 ms | ±0.01 | 1.04 ms | 240,000 rps (techo: solo 202,336) |
| ray-thr | = 160,000 rps (0.80x líder) | 0.60 ms | 0.73 ms | ±0.00 | 1.17 ms | 200,000 rps (techo: solo 161,595) |
| ray | = 160,000 rps (0.80x líder) | 0.48 ms | 0.74 ms | ±0.00 | 1.05 ms | 200,000 rps (techo: solo 188,087) |
| chi | 120,000 rps (0.60x líder) | 0.65 ms | 2.23 ms | ±0.00 | 2.77 ms | 160,000 rps (techo: solo 124,776) |
| express | 40,000 rps (0.20x líder) | 2.44 ms | 4.81 ms | ±0.03 | 5.28 ms | 80,000 rps (techo: solo 40,046) |

