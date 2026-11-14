"""Generated loopback-only native guard checks; the HLS rewriter is bypassed."""

import argparse
import functools
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import ctypes as C

sys.path.insert(0, str(Path(__file__).resolve().parent))
import smoke  # noqa: E402


class CountServer(ThreadingHTTPServer):
    daemon_threads = True
    connections = 0

    def get_request(self):
        request = super().get_request()
        self.connections += 1
        return request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    backend = parser.add_mutually_exclusive_group(required=True)
    backend.add_argument("--probe", type=Path, help="Native probe executable or simulator launcher")
    backend.add_argument("--library", type=Path, help="Exact libmpv shared library")
    parser.add_argument("--ffmpeg", required=True, type=Path)
    parser.add_argument("--baseline", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="patina-native-relay-") as temp:
        directory = Path(temp)

        class Forbidden(SimpleHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.end_headers()
                self.wfile.write(bytes(range(16)))

            def log_message(self, *_):
                pass

        forbidden = CountServer(("127.0.0.1", 0), Forbidden)
        proxy = CountServer(("127.0.0.1", 0), Forbidden)
        foreign = f"127.0.0.1:{forbidden.server_port}"

        class Relay(SimpleHTTPRequestHandler):
            def do_GET(self):
                prefix = "/generated-session-capability/"
                if not self.path.startswith(prefix):
                    self.server.outside_scope += 1
                    self.send_error(403)
                    return
                path = self.path[len(prefix) :]
                if path in ("redirect.ts", "redirect-tls.ts"):
                    target = ("https" if path == "redirect-tls.ts" else "http") + "://" + foreign + "/segment.ts"
                    self.send_response(302)
                    self.send_header("Location", target)
                    self.end_headers()
                    return
                self.path = "/" + path
                super().do_GET()

            def log_message(self, *_):
                pass

        relay = CountServer(("127.0.0.1", 0), functools.partial(Relay, directory=str(directory)))
        relay.outside_scope = 0
        prefix = f"http://127.0.0.1:{relay.server_port}/generated-session-capability/"
        servers = [relay, forbidden, proxy]
        threads = [threading.Thread(target=s.serve_forever, daemon=True) for s in servers]
        for thread in threads:
            thread.start()
        try:
            smoke.generate(args.ffmpeg.resolve(), directory)
            (directory / "key.bin").write_bytes(bytes(range(16)))
            (directory / "key-info").write_text(prefix + "key.bin\n" + str(directory / "key.bin") + "\n" + "00" * 16 + "\n")
            subprocess.run(
                [
                    str(args.ffmpeg.resolve()),
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-nostdin",
                    "-y",
                    "-i",
                    str(directory / "generated.ts"),
                    "-c",
                    "copy",
                    "-f",
                    "hls",
                    "-hls_time",
                    "1",
                    "-hls_list_size",
                    "0",
                    "-hls_playlist_type",
                    "vod",
                    "-hls_key_info_file",
                    str(directory / "key-info"),
                    "-hls_segment_filename",
                    str(directory / "encrypted-%03d.ts"),
                    str(directory / "encrypted.m3u8"),
                ],
                check=True,
                timeout=120,
            )
            cases = {
                "foreign-key": f"#EXT-X-KEY:METHOD=AES-128,URI=http://{foreign}/key.bin\n#EXTINF:1,\n{prefix}encrypted-000.ts\n",
                "foreign-tls-key": f'#EXT-X-KEY:METHOD=AES-128,URI="https://{foreign}/key.bin"\n#EXTINF:1,\n{prefix}encrypted-000.ts\n',
                "foreign-segment": f"#EXTINF:1,\nhttp://{foreign}/segment.ts\n",
                "foreign-tcp": f"#EXTINF:1,\ntcp://{foreign}/segment.ts\n",
                "foreign-file": f"#EXTINF:1,\nfile://{directory}/segment-000.ts\n",
                "other-session": f"#EXTINF:1,\nhttp://127.0.0.1:{relay.server_port}/other-session/segment.ts\n",
                "redirect": f"#EXTINF:1,\n{prefix}redirect.ts\n",
                "redirect-tls": f"#EXTINF:1,\n{prefix}redirect-tls.ts\n",
            }
            for name, body in cases.items():
                (directory / (name + ".m3u8")).write_text("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n" + body + "#EXT-X-ENDLIST\n")
            env = {k: v for k, v in os.environ.items() if k.lower() not in ("http_proxy", "https_proxy", "all_proxy", "no_proxy")}
            variants = [{}, {"no_proxy": "127.0.0.1"}, {"NO_PROXY": "*"}, {"no_proxy": "*", "NO_PROXY": "127.0.0.1"}]
            for index, variant in enumerate(variants[:1] if args.baseline else variants):
                child = {**env, "http_proxy": f"http://127.0.0.1:{proxy.server_port}", "https_proxy": f"http://127.0.0.1:{proxy.server_port}", **variant}
                if args.baseline:
                    child["no_proxy"] = "127.0.0.1"
                for name in ["foreign-key"] if args.baseline else ["generated", "encrypted", *cases]:
                    before = (forbidden.connections, proxy.connections)
                    success = name in ("generated", "encrypted")
                    command = [str(args.probe.resolve())] if args.probe else [sys.executable, str(Path(__file__).resolve()), "--native-child", str(args.library.resolve())]
                    result = subprocess.run(
                        [*command, prefix + name + ".m3u8", prefix, "unguarded" if args.baseline else "guarded", "success" if success else "refusal"],
                        env=child,
                        capture_output=True,
                        text=True,
                        timeout=20,
                    )
                    counts = (forbidden.connections - before[0], proxy.connections - before[1])
                    if args.baseline:
                        if counts[0] < 1:
                            raise RuntimeError("baseline failed to reproduce native forbidden-server access")
                        print("REPRODUCED: stock native demuxer reached forbidden loopback server with no_proxy", flush=True)
                    elif result.returncode or counts != (0, 0) or relay.outside_scope:
                        raise RuntimeError(f"{name}, environment {index}: exit={result.returncode}, forbidden/proxy connections={counts}; {result.stderr.strip()}")
                    else:
                        print(f"PASS {name}, proxy environment {index}: forbidden/proxy connections=0/0", flush=True)
        finally:
            for server in servers:
                server.shutdown()
                server.server_close()
            for thread in threads:
                thread.join(timeout=5)


def native_child(arguments):
    library, url, prefix, guarded, expectation = arguments
    runtime = smoke.Runtime(Path(library))
    handle = runtime.dll.mpv_create()
    if not handle:
        return 3
    try:
        options = {"config": "no", "terminal": "no", "vo": "null", "ao": "null", "hwdec": "no", "idle": "yes", "keep-open": "no", "cache": "no", "demuxer": "lavf"}
        if guarded == "guarded":
            options.update({"demuxer-lavf-propagate-opts": "yes", "demuxer-lavf-o": "patina_http_prefix=" + prefix, "stream-lavf-o": "patina_http_prefix=" + prefix})
        for key, value in options.items():
            runtime.check(runtime.dll.mpv_set_option_string(handle, key.encode(), value.encode()), key)
        runtime.check(runtime.dll.mpv_initialize(handle), "initialize")
        if guarded == "guarded" and "patina-relay-guard-v1" not in (runtime.text(handle, "protocol-list") or ""):
            raise RuntimeError("native relay guard is absent")
        command = (C.c_char_p * 4)(b"loadfile", url.encode(), b"replace", None)
        runtime.check(runtime.dll.mpv_command(handle, command), "loadfile")
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            event = runtime.dll.mpv_wait_event(handle, 0.05).contents
            position = runtime.position(handle)
            if position is not None and position > 0.15:
                return 0 if expectation == "success" else 4
            if event.event_id == 7:
                ended = C.cast(event.data, C.POINTER(smoke.EndFile)).contents
                # Lavf may turn an unreadable HLS key/segment into EOF. Both
                # terminal results must stop without advancing media; the
                # parent separately requires zero forbidden connections.
                return 0 if expectation == "refusal" and ended.reason in (0, 4) else 5
        return 6
    finally:
        runtime.dll.mpv_terminate_destroy(handle)


if __name__ == "__main__":
    if sys.argv[1:2] == ["--native-child"]:
        sys.exit(native_child(sys.argv[2:]))
    main()
