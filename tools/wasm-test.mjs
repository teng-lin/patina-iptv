// Runs a Rust test executable built for wasm32-unknown-unknown in Node,
// without wasm-bindgen: the browser store's tests
// (crates/iptv-store-sqlite/tests/it, see its wasm.rs for the imports).
//
//   cargo test -p iptv-store-sqlite --target wasm32-unknown-unknown --test it --no-run
//   node tools/wasm-test.mjs target/wasm32-unknown-unknown/debug/deps/it-*.wasm
//
// Use `--lib` for the unit tests. Compiling SQLite needs a clang with the
// wasm32 target. For the 2020 TV's feature set (MVP with sign-extension and
// mutable globals), build with the pinned nightly instead:
//
//   CFLAGS_wasm32_unknown_unknown="-mcpu=mvp -msign-ext -mmutable-globals" \
//   RUSTFLAGS="-C target-cpu=mvp -C target-feature=+sign-ext,+mutable-globals" \
//   cargo +"$(tools/ci/fuzz-imports.sh --toolchain)" test -Zbuild-std=std,panic_abort,test \
//     -p iptv-store-sqlite --target wasm32-unknown-unknown --test it --no-run
//
// The target has no stdout and aborts on the first panic, so libtest's
// report is invisible: the executable's panic hook sends the message here,
// and the exit status says whether every test passed.
import fs from 'node:fs';
import crypto from 'node:crypto';

const path = process.argv[2];
if (!path) {
  console.error('usage: node tools/wasm-test.mjs TEST.wasm');
  process.exit(2);
}
let memory;
const text = (ptr, len) => new TextDecoder().decode(new Uint8Array(memory.buffer, ptr, len));
const env = {
  test_report: (ptr, len) => console.error(text(ptr, len)),
  test_now_ms: () => Date.now(),
  test_random: (ptr, len) => crypto.getRandomValues(new Uint8Array(memory.buffer, ptr, len)),
};
const module = new WebAssembly.Module(fs.readFileSync(path));
const instance = new WebAssembly.Instance(module, { env });
memory = instance.exports.memory;
instance.exports.test_install_panic_hook();
const started = performance.now();
try {
  instance.exports.main(0, 0);
} catch (error) {
  console.error(`${path}: a test failed (${error.message})`);
  process.exit(1);
}
console.log(`${path}: all tests passed in ${((performance.now() - started) / 1000).toFixed(1)} s`);
