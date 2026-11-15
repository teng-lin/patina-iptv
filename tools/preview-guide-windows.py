#!/usr/bin/env python3
"""Run the actual Patina guide with synthetic channels in isolated temporary storage."""
import argparse
import functools
import http.server
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
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
    if sys.platform not in ('linux', 'darwin'):
        raise SystemExit('This prototype runs on Linux and macOS.')
    platform = 'macos' if sys.platform == 'darwin' else 'linux'
    runtime = Path(os.environ.get('IPTV_RS_MPV_LIB_DIR',
                                  ROOT / f'target/mpv-runtime-{platform}/lib')).resolve()
    library = 'libmpv.2.dylib' if platform == 'macos' else 'libmpv.so.2'
    if not (runtime / library).exists():
        raise SystemExit(f'Fetch the pinned {platform} player runtime first; see README.md, '
                         'or set IPTV_RS_MPV_LIB_DIR to its lib directory.')
    if not all((ROOT / f'fixtures/hls/ch{i}/index.m3u8').exists() for i in range(3)):
        raise SystemExit('Generate synthetic streams first: bash tools/make-hls-fixtures.sh')
    temporary = ROOT / 'target/patina-test-tmp'
    temporary.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, TMPDIR=str(temporary), IPTV_RS_MPV_LIB_DIR=str(runtime),
               LIBRARY_PATH=str(runtime), IPTV_RS_AUDIO='null')
    env['DYLD_LIBRARY_PATH' if platform == 'macos' else 'LD_LIBRARY_PATH'] = str(runtime)
    if not args.no_build:
        subprocess.run(['cargo','build','-p','iptv-app','--features','test-support','--example','guide_window_preview'], cwd=ROOT,env=env,check=True)
    target = Path(env.get('CARGO_TARGET_DIR', ROOT / 'target'))
    binary = (target / 'debug/examples/guide_window_preview').resolve()
    if platform == 'macos':
        # A private app bundle gives native inspection tools an unambiguous
        # app identity. Execute it directly so the isolated HOME and DYLD
        # environment reach it; never launch an installed Patina copy.
        contents = target / 'guide-window-preview.app/Contents'
        executable = contents / 'MacOS/guide_window_preview'
        executable.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(binary, executable)
        with (contents / 'Info.plist').open('wb') as info:
            plistlib.dump({'CFBundleIdentifier': 'com.hantekllc.iptvrs.guidepreview',
                          'CFBundleExecutable': executable.name,
                          'CFBundleName': 'Patina Synthetic Preview',
                          'CFBundlePackageType': 'APPL',
                          'NSHighResolutionCapable': True}, info)
        binary = executable.resolve()
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
