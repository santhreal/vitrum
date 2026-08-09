# Performance

Every number here is measured by `harness/` on a real host and written by
`make perf-tables`. CI fails if a figure drifts from the snapshot it came from.

## Memory

<!-- BENCH:footprint:start -->
Measured on **13th Gen Intel(R) Core(TM) i9-13900K**, 32 logical cores, WebKitGTK 2.52.3-0ubuntu0.24.04.1, `Linux 6.8.0-136-generic x86_64`. Every window holds a live shell against one `vitrum-server`. The figure is PSS, which charges shared pages once, so the totals below add up across processes instead of counting the same engine twice. `vitrum 0.1.0` at `18df8cb`.

| Windows | Client tree | Client processes | Daemon tree | Daemon processes |
|---:|---:|---:|---:|---:|
| 1 | 247.8 MB | 3 | 5.5 MB | 2 |
| 20 | 460.1 MB | 3 | 40.6 MB | 21 |

The 20-window client tree is still 3 processes, not 60: every window is a view onto one shared web process and one network process. Going from 1 to 20 windows costs **11.2 MB per extra window**.

Where the 20-window client tree goes:

| Process | Count | PSS |
|---|---:|---:|
| `WebKitWebProcess` | 1 | 298.0 MB |
| `vitrum` | 1 | 140.8 MB |
| `WebKitNetworkProcess` | 1 | 21.3 MB |

The daemon side of the same run is 40.6 MB across 21 processes, and the shells the operator asked for are most of it:

| Process | Count | PSS |
|---|---:|---:|
| `bash` | 20 | 35.0 MB |
| `vitrum-server` | 1 | 5.6 MB |

Reproduce: `harness/run.sh memory 1` and `harness/run.sh memory 20`, then `make perf-tables`.
<!-- BENCH:footprint:end -->

## Idle cost

<!-- BENCH:idle:start -->
An idle terminal should cost nothing. Measured over 60 s with 20 windows open, every one holding a live shell, on **13th Gen Intel(R) Core(TM) i9-13900K**, 32 logical cores, WebKitGTK 2.52.3-0ubuntu0.24.04.1, `Linux 6.8.0-136-generic x86_64`.

| Tree | CPU | PSS before | PSS after | Drift |
|---|---:|---:|---:|---:|
| Client | 0.1000% of one core | 447.4 MB | 447.4 MB | +0.0 MB |
| Daemon | 0.0000% of one core | 40.7 MB | 40.7 MB | +0.0 MB |

That is 6 scheduler ticks in 60 seconds across 20 windows. Nothing polls, so nothing accumulates: the drift is the point of the last column.

Reproduce: `harness/run.sh idle-cpu 60 20`, then `make perf-tables`.
<!-- BENCH:idle:end -->

Hardware rendering lowers the idle figure. Agents printing raise it.

## Cold start

Measured on the same host, single window: the web process exists 0.20 s after
exec, and the window is painted and idle at 1.31 s. Three runs: 1.26, 1.31,
1.36.
