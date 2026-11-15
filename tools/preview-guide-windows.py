#!/usr/bin/env python3
"""Run the actual Patina guide with synthetic channels in isolated temporary storage."""
import argparse
import functools
import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[1]

class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--no-build', action='store_true')
    parser.add_argument('--walkthrough', action='store_true')
    args = parser.parse_args()
    runtime = ROOT / 'target/mpv-runtime-linux/lib'
    if not (runtime / 'libmpv.so.2').exists():
        raise SystemExit('Fetch the pinned Linux player runtime first; see README.md.')
    temporary = ROOT / 'target/patina-test-tmp'
    temporary.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, TMPDIR=str(temporary), LD_LIBRARY_PATH=str(runtime), IPTV_RS_AUDIO='null')
    if not args.no_build:
        subprocess.run(['cargo','build','-p','iptv-app','--features','test-support','--example','guide_window_preview'], cwd=ROOT,env=env,check=True)
    binary = ROOT / 'target/debug/examples/guide_window_preview'
    server = http.server.ThreadingHTTPServer(('127.0.0.1',18091),functools.partial(QuietHandler,directory=str(ROOT/'fixtures/hls')))
    threading.Thread(target=server.serve_forever,daemon=True).start()
    with tempfile.TemporaryDirectory(prefix='guide-windows-',dir=temporary) as home:
        env.update(HOME=home, LOCALAPPDATA=home)
        subprocess.run([str(binary),'seed'],env=env,check=True)
        if args.walkthrough:
            env['PATINA_GUIDE_WINDOW_WALKTHROUGH']='1'
            keys = json.loads((Path(home) / 'demo-channel-keys.json').read_text())
            env['PATINA_GUIDE_SECOND_KEY'] = keys[1]
            env['PATINA_GUIDE_THIRD_KEY'] = keys[2]
        process = subprocess.Popen([str(binary)],cwd=ROOT,env=env)
        print(f'Synthetic actual-App demo running (PID {process.pid}).',flush=True)
        try:
            process.wait()
        except KeyboardInterrupt:
            process.terminate()
            process.wait()
        finally:
            server.shutdown()
            server.server_close()

if __name__ == '__main__':
    main()
