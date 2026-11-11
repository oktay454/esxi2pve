# Transfer engine

[English](architecture.md) | [Türkçe](architecture.tr.md)

The delta transfer engine applies selected logical regions of a frozen source disk chain to a local raw file or block device. QEMU decodes the disk format; Rust manages source reads, target writes, SHA-256 verification and durable verification records. QEMU performs the initial full copy and comparison.

## Workflow

```text
Frozen source + validated descriptor/CID chain
    ↓
Split nonoverlapping logical ranges into chunks of at most 4 MiB
    ↓
Multiple asynchronous reads on one NBD connection
    ↓
Data queue bounded by a shared memory budget
    ↓
Source SHA-256 → positional target write
    ↓
Group chunks whose writes have completed
    ↓
fsync → target readback → SHA-256 comparison
    ├─ Match → append verification records and fsync
    └─ Mismatch → reread source; does its original hash still match?
                      ├─ No → fail
                      └─ Yes → rewrite once, fsync, reread and verify
    ↓
Entire plan durably verified, no pending work → complete
```

The source event loop multiplexes nonblocking requests and replies over one NBD connection. Request handles map out-of-order replies to their correct offsets. A separate writer thread hashes source data and performs positional writes. A separate persistence worker handles syncing, readback and the verification journal. No additional async runtime is required.

These stages overlap across different groups. While a previous group is being verified, subsequent reads and writes to disjoint offsets can progress. Each range has one owner within a round. Disks and snapshot rounds complete in dependency order; different source generations are never concurrently applied to the same target offsets. Zero regions from the QEMU map are included in the plan.

## Limits and I/O balance

| Setting | Default |
|---|---:|
| Source read chunk | At most 4 MiB |
| Initial outstanding reads | 2; 1 if the configured maximum is 1 |
| Maximum read depth | 16; `--io-max-depth` accepts 1..64 |
| Data buffer budget | 128 MiB; `--io-memory-mib` accepts 16..1024 MiB |
| Persistence group | At most 64 MiB or 4096 chunks |
| Concurrent persistence per target | 1 |
| Source NBD read timeout | 120 seconds; configurable through the library |
| Readback | Direct I/O; `--verify-read direct` |

The writer also submits a partial group after 100 ms without another chunk. All writes belonging to a group must finish before its sync. Verification records are appended and synced after the whole group verifies. Directory entries for new journal files are also synced. Success and progress count only data with completed durable verification records.

The memory budget bounds outstanding source reads, replies and queued write payloads, and reserves space for readback and a retry within the same budget. Group metadata is separately bounded by chunk count. This option does not cap total process memory or the kernel page cache.

The balanced controller uses one-second windows of target write/readback latency, `fsync` duration, queue buildup and, when available, device queue and Linux I/O pressure (PSI). Two consecutive busy windows halve the new-read depth and introduce a 5–100 ms submission delay. After three suitable stable windows it first reduces that delay, then tries increasing depth by one. A depth trial that does not improve throughput by at least 5% is reverted. Active requests are allowed to finish; reductions restrict new submissions.

Local device statistics and PSI cannot reveal all workloads from other nodes sharing a SAN/storage system. Unavailable measurements are JSON `null`. This mechanism responds conservatively to observed pressure; it cannot guarantee zero latency impact on other VMs.

## Durability and readback

Default verification uses Linux `O_DIRECT` to bypass the kernel page cache. Safe Rust provides aligned buffers and supports sector-sized small chunks and disk tails. This read path is probed after target allocation and before source shutdown. Unsupported direct readback stops the operation without silently falling back.

Explicit `--verify-read buffered` uses normal readback and records a warning. Reads may be satisfied from the page cache. Both modes wait for `fsync` first; guarantees depend on the storage device's reported persistence behavior. Direct reads do not physically test power-loss behavior.

## Errors and controlled retries

A checksum mismatch retries only the affected chunk, once. The source is reread and checked against the original source hash. If the source changed, the expected hash is preserved and the operation fails. Otherwise the target is rewritten, synced and verified again. A second mismatch fails. Each disk/delta round permits at most three such retries; a fourth mismatch stops the operation. The first mismatch and retry outcome have separate durable records.

Device read/write errors, source-chain errors, full disks, permission errors and `fsync` errors are not automatically retried. Descriptor-chain fingerprints are compared before and after transfer. Do not modify the frozen chain using other tools during migration.

Errors stop new submissions. The function joins all local writer and verification workers before returning, leaving no unowned background target writes. Kernel-blocked I/O can delay that return. A timeout does not imply cancellation of underlying work. Partial targets and diagnostic records are retained.

## Machine-readable records

Verification files such as `final-ranges-0.verified.jsonl` contain:

- `offset`, `length`, `sha256`: the verified logical range and original source hash.
- `attempt`: 0 for the initial transfer, 1 for a successful retry.
- `verification`: `direct` or `buffered`.

Sidecars use names such as `final-ranges-0.verified.metrics.jsonl` and `final-ranges-0.verified.retries.jsonl`. Metrics include planned/verified bytes, read requests, peak outstanding requests/buffer usage, depth, retries and stage durations. One-second records also contain local pressure, submission delay and received source bytes. Duration fields ending in `_us` are accumulated microseconds; overlapping reads mean these totals do not equal wall time. `elapsed_ms` is wall time.

These records do not implement automatic resume. Existing source snapshot chains are rejected at job start. Snapshots created by the application are retained after success. No independent final full-disk hash scan runs automatically. Starting the target and confirming application health are separate checks.
