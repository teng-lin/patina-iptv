# Rust review of iptv-net

This native library implements request transport, playback relays and recording streams. Its redirect, manifest and capability boundaries are well defended, but the recording-file server reintroduces an unchecked filesystem-open race and has no deadline while serving bodies. Address those gaps before transport API polish. Clippy passed in the coordinated workspace run; see validation below.

## Must fix

### iptv-net-001 Recording file validation does not protect the opened handle

**Location:** `crates/iptv-net/src/recording_file.rs:76`, `:80`; related resume open in `crates/iptv-net/src/recording.rs:142`.

**Rule:** Validate resources after acquisition and preserve ownership invariants; rust-best-practices sections 3 and 8, plus the project's private-file/symlink refusal contract.

**Problem:** The server checks `symlink_metadata(path)` and later opens the same pathname with ordinary File::open. If the path is replaced between those operations, the opened object can be a symlink target or FIFO. The opened metadata is used only for its length, without checking that it is a regular file. The resume writer uses O_NOFOLLOW on Unix but still lacks O_NONBLOCK and a descriptor-type check before mutating the file. A FIFO swapped into either path can block a Tokio blocking worker (or the synchronous writer) indefinitely. The recording-file case can also serve a replacement file instead of the approved recording. The threat requires mutation of the private media directory; this is not an unauthenticated arbitrary-file-read claim.

**Fix proposal:** Reuse the regular-file opening discipline already present in `src/file.rs`, adding no-follow protection and descriptor validation. On Unix:

```rust
let mut options = tokio::fs::OpenOptions::new();
options.read(true);
#[cfg(unix)]
options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
let file = options.open(&path).await?;
let meta = file.metadata().await?;
if !meta.is_file() {
    return Err(std::io::ErrorKind::PermissionDenied.into());
}
```

Apply corresponding nonblocking regular-file validation before resume truncation/writes. Use the existing platform private-file policy for Windows reparse points, rather than assuming Unix flags cover that host. **Regression:** In the existing harness, synthetic file-to-symlink/FIFO swap tests, with helper-level synchronization so the race is deterministic; no hung runtime workers after timeout. **Disposition:** open; confirmed check/open gap by source analysis, race not dynamically reproduced.

### iptv-net-002 Stalled recording-file clients retain every serving slot

**Location:** `crates/iptv-net/src/recording_file.rs:32`, `:108`, `:111`.

**Rule:** Bound resource lifetime as well as count; rust-best-practices section 7 concurrency and the transport's bounded-consumer design.

**Problem:** Only reading the request header has a timeout. The response header and `tokio::io::copy` body writes have none. Four capability-authorized clients that request a sufficiently large local recording and stop reading eventually block all four tasks on full send buffers. Every later player request is dropped while those sockets remain open; owner teardown is the only guaranteed cancellation. The main playback/live relays already use a 15-second write deadline, but this server does not.

**Fix proposal:** Copy in bounded chunks and apply the existing 15-second idle write policy to each header/body write, allowing healthy long playback to continue. Avoid a fixed total timeout for the entire recording.

```rust
async fn send(stream: &mut tokio::net::TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    tokio::time::timeout(Duration::from_secs(15), stream.write_all(bytes)).await?
}
// Replace unbounded copy with read-limited chunks followed by send(...).
```

**Regression:** Existing recording transport harness: hold four capability-authenticated GET clients without consuming a large synthetic body, advance/test the write deadline, then prove a new range request succeeds. Include a healthy slow reader and shutdown cancellation. **Disposition:** open; reproduced using the exact `recording_file.rs` source outside the repository. Four clients read completed response headers for a synthetic sparse 128 MiB file and then stopped consuming. After 18 seconds, a fresh range GET received zero bytes and EOF while the four earlier sockets remained open. Reproduction source/output: `/tmp/patina-rust-review-20261116/net-slow-recording-probe.rs` and `net-slow-recording-probe.txt`. The 15-second comparison is the existing playback/live write policy, not a currently documented recording-file timeout.

## Should fix

No additional independent finding retained.

## Consider

POST body replay on a cross-origin 307/308 is intentional and tested. If future provider authentication uses POST bodies, explicitly decide whether those bodies may cross origins before reusing the generic request path. Current header scoping alone cannot redact form credentials; no current provider POST flow was found.

## What's already good

- Redirects strip arbitrary credential headers through an allowlist, reject HTTPS downgrade and pin vetted DNS answers.
- HLS rewriting addresses differences between FFmpeg and Media3 parsers; DASH rewriting rejects dangerous encoding and remote-clock paths.
- Random relay capabilities, constant-time comparison and unauthenticated connection deadlines protect local admission.
- Playback cookies are bounded and scoped to the exact origin, and transport errors avoid raw provider URLs.

## Baseline and validation

- Review baseline: `0e197073a272eceb0631e4d53b08d2823950c416`; Rust edition 2024, MSRV/toolchain 1.99.1; unpublished workspace crate.
- Review only: proposed code appears in this report; no implementation, test, manifest or lockfile changes were made.
- Parent-coordinated workspace Clippy (`--locked --workspace --all-targets --features iptv-runtime/test-support,iptv-app/test-support -- -D warnings`) passed.
- Full `tools/check.sh` stops at the pre-existing NSH pin mismatch: checkout `32769658ad34ad364b960a134cc73c754d50f2d9`, pinned `9ed782cbf362ab63ba18c47dc657fe944c405e50`. Workspace test results are recorded centrally by the coordinator; this report does not claim a separate crate test/fuzz/device run.
- Synthetic source/linked-library probes were built with `rustc`, outside the repository. Linked probes and output: `/tmp/patina-rust-review-20261116/ingestion-linked-probes.rs` and `ingestion-linked-probes.txt` on the review host. These temporary paths are evidence pointers, not portable regression tests.

## File coverage

Production modules were read for contracts and defect paths; inline tests were read where relevant. Integration files were inventoried by scenario and relevant regression cases inspected. “Inventoried” does not mean every test body was audited. No build script exists in this crate.

| File | Coverage |
|---|---|
| `crates/iptv-net/Cargo.toml` | Manifest reviewed |
| `crates/iptv-net/examples/import.rs` | Example reviewed |
| `crates/iptv-net/src/body.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/client.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/cross_origin.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/error.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/file.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/lib.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/live/tests.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/live.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/playback/dash.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/playback/hls.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/playback.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/public_resolver.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/recording.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/recording_file.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/redirect.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/src/tls.rs` | Source reviewed; inline tests inspected where relevant |
| `crates/iptv-net/tests/it/audit_transport.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-net/tests/it/log_redaction.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-net/tests/it/main.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-net/tests/it/net.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-net/tests/it/pipeline.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-net/tests/it/playback.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-net/tests/it/recording.rs` | Scenario inventory and targeted test inspection |
| `crates/iptv-net/tests/it/support/mod.rs` | Scenario inventory and targeted test inspection |

Platform and feature coverage is by source inspection only unless explicit runtime evidence appears above. Native device/fuzz validation remains a fix-stage gate.

## Resolution tracking

| Finding | Disposition | Implementation | Required evidence |
|---|---|---|---|
| `iptv-net-001` | Open | No code changed | Proposed regression and affected crate/caller checks above |
| `iptv-net-002` | Open | No code changed | Proposed regression and affected crate/caller checks above |
