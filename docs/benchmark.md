# Benchmark protocol

The governor's value is an empirical claim about responsiveness, not an assumption that serial work
always finishes faster.

Run the same repositories and warm/cold cache policy across these scenarios, rotating their order:

1. one workload at a time;
2. two simultaneous workloads;
3. four and six simultaneous workloads without governance;
4. four and six submitted with capacity 1;
5. four and six submitted with capacity 2.

Measure:

- first completion, mean completion, and makespan;
- queue time;
- p50/p95 latency of a light sentinel every two seconds;
- CPU, RSS, load, memory pressure, swap, page faults, processes, and threads;
- observable CPU of endpoint products without changing them;
- perceived Cursor typing/window responsiveness.

Choose capacity lexicographically:

1. eliminate any capacity that violates sentinel, responsiveness, memory-pressure, or swap SLOs;
2. among the remaining capacities, prefer the better mean/makespan balance;
3. if evidence is inconclusive, keep capacity 1.

Document the target Mac model, RAM, macOS build, agent/RTK versions, repositories, workload hashes,
cache policy, sample count, raw measurements, and confidence intervals.
