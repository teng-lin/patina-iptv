// Runs Rust test executables built for wasm32-unknown-unknown in Node,
// without wasm-bindgen: the browser store's tests (crates/iptv-store-sqlite;
// see tests/it/wasm.rs for the imports they need).
//
//   node tools/wasm-test.mjs TEST.wasm ...
//   node tools/wasm-test.mjs --cargo --locked -p iptv-store-sqlite --lib --test it
//
// With `--cargo`, the remaining arguments go to
// `cargo test --target wasm32-unknown-unknown --no-run`, and every
// test executable it builds runs. Compiling SQLite needs a clang with the
// wasm32 target. For the 2020 TV's feature set (MVP with sign-extension and
// mutable globals), build with the pinned nightly instead:
//
//   CFLAGS_wasm32_unknown_unknown="-mcpu=mvp -msign-ext -mmutable-globals" \
//   RUSTFLAGS="-C target-cpu=mvp -C target-feature=+sign-ext,+mutable-globals" \
//   cargo +"$(tools/ci/fuzz-imports.sh --toolchain)" test -Zbuild-std=std,panic_abort,test \
//     -p iptv-store-sqlite --target wasm32-unknown-unknown --test it --no-run
//
// The target has no stdout and aborts on the first panic, so libtest's
// report, test names and count included, is invisible: the executable's
// panic hook sends a failure's message here, and libtest's exit status
// (main's return value, or a trap for a panic) says whether every test
// passed.
import { execFileSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';

function executables(args) {
  const out = execFileSync(
    'cargo',
    ['test', '--target', 'wasm32-unknown-unknown', '--no-run', '--message-format=json', ...args],
    { stdio: ['ignore', 'pipe', 'inherit'], maxBuffer: 1 << 28 },
  );
  return out
    .toString()
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .filter((m) => m.reason === 'compiler-artifact' && m.executable)
    .map((m) => m.executable);
}

function run(path) {
  let memory;
  const text = (ptr, len) => new TextDecoder().decode(new Uint8Array(memory.buffer, ptr, len));
  const env = {
    test_report: (ptr, len) => console.error(text(ptr, len)),
    test_now_ms: () => Date.now(),
    test_random: (ptr, len) => {
      // getRandomValues fills at most 64 KiB per call.
      for (let at = 0; at < len; at += 65536) {
        crypto.getRandomValues(new Uint8Array(memory.buffer, ptr + at, Math.min(65536, len - at)));
      }
    },
  };
  const instance = new WebAssembly.Instance(new WebAssembly.Module(fs.readFileSync(path)), { env });
  memory = instance.exports.memory;
  instance.exports.test_install_panic_hook();
  const started = performance.now();
  let status;
  try {
    status = instance.exports.main(0, 0);
  } catch (error) {
    console.error(`${path}: a test failed (${error.message})`);
    return false;
  }
  if (status !== 0) {
    console.error(`${path}: libtest exited with status ${status}`);
    return false;
  }
  console.log(`${path}: all tests passed in ${((performance.now() - started) / 1000).toFixed(1)} s`);
  return true;
}

const args = process.argv.slice(2);
const paths = args[0] === '--cargo' ? executables(args.slice(1)) : args;
if (paths.length === 0) {
  console.error('usage: node tools/wasm-test.mjs (TEST.wasm ... | --cargo CARGO-TEST-ARGS ...)');
  process.exit(2);
}
let passed = true;
for (const path of paths) {
  passed = run(path) && passed;
}
console.log(`${paths.length} test executables, ${passed ? 'all passed' : 'some failed'}`);
process.exit(passed ? 0 : 1);
