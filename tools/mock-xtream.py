#!/usr/bin/env python3
"""Local mock Xtream Codes panel for development (synthetic data only).

Answers player_api.php actions with invented JSON and redirects stream,
movie, episode and timeshift URLs to the synthetic HLS channels served by
tools/mock-server.py (fixtures/hls/ch0..ch2). Wrong credentials get
{"user_info":{"auth":0}}, like real panels.

  python3 -I tools/mock-xtream.py                      # :8090, user demo / demo
  python3 -I tools/mock-xtream.py --port 8090 --channels 2000 --status Expired
  # emulator: server http://10.0.2.2:8090, username demo, password demo

Actions: (none), get_account_info, get_profile, get_live_categories,
get_live_streams, get_vod_categories, get_vod_streams, get_vod_info,
get_series_categories, get_series, get_series_info, get_short_epg,
get_simple_data_table (both spellings). Also /get.php (m3u_plus) and
/xmltv.php (fixtures/hls/iptv/guide.xml.gz when present, else a small
generated guide). Passwords are redacted in the request log.
"""

import argparse
import base64
import gzip
import http.server
import json
import os
import re
import sys
import time
import urllib.parse

ARGS = None
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Credentials in logged request lines, whatever their value.
REDACT_QUERY = re.compile(r"\b(username|password)=[^&\s]*")
REDACT_PATH = re.compile(r"/(live|movie|series|timeshift)/[^/\s]+/[^/\s]+/")
CATEGORIES = ["News", "Sports", "Movies", "Kids", "Music", "Documentary", "Science", "Travel"]


def live_categories():
    return [{"category_id": str(i + 1), "category_name": n, "parent_id": 0} for i, n in enumerate(CATEGORIES)]


def live_streams(category=None):
    out = []
    for i in range(1, ARGS.channels + 1):
        cat = str((i - 1) % len(CATEGORIES) + 1)
        if category and category != cat:
            continue
        out.append({
            "num": i,
            "name": f"Mock {CATEGORIES[int(cat) - 1]} {i:04d}",
            "stream_type": "live",
            "stream_id": i,
            "stream_icon": f"http://{ARGS.public}/logo/{i}.png",
            "epg_channel_id": f"ch{i}.synthetic.example",
            "added": "1700000000",
            "category_id": cat,
            "category_ids": [int(cat)],
            "custom_sid": "",
            "tv_archive": 1 if i % 5 == 0 else 0,
            "direct_source": "",
            "tv_archive_duration": 3 if i % 5 == 0 else 0,
        })
    return out


def vod_streams(category=None):
    out = []
    for i in range(1, ARGS.vod + 1):
        cat = str(100 + (i - 1) % 4)
        if category and category != cat:
            continue
        out.append({
            "num": i, "name": f"Mock Film {i:03d}", "stream_type": "movie",
            "stream_id": 10000 + i, "stream_icon": "", "rating": "6.5",
            "added": "1700000000", "category_id": cat, "container_extension": "mp4",
            "direct_source": "",
        })
    return out


def series_list(category=None):
    out = []
    for i in range(1, ARGS.series + 1):
        cat = str(200 + (i - 1) % 3)
        if category and category != cat:
            continue
        out.append({
            "num": i, "name": f"Mock Serial {i:03d}", "series_id": 20000 + i,
            "cover": "", "plot": "An invented serial.", "rating": "7",
            "last_modified": "1700000000", "category_id": cat,
        })
    return out


def account():
    now = int(time.time())
    exp = now - 3600 if ARGS.status == "Expired" else now + 30 * 86400
    return {
        "user_info": {
            "username": ARGS.user, "password": ARGS.password, "message": "Synthetic mock panel",
            "auth": 1, "status": ARGS.status, "exp_date": str(exp), "is_trial": "0",
            "active_cons": "0", "created_at": "1700000000", "max_connections": "1",
            "allowed_output_formats": ["m3u8", "ts"],
        },
        "server_info": {
            "url": ARGS.public.split(":")[0], "port": ARGS.public.split(":")[-1],
            "https_port": "", "server_protocol": "http", "timezone": "UTC",
            "timestamp_now": now, "time_now": time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(now)),
        },
    }


def b64(s):
    return base64.b64encode(s.encode()).decode()


def short_epg(stream_id, limit, table=False):
    now = int(time.time()) // 3600 * 3600
    rows = []
    for k in range(-6 if table else 0, limit):
        start = now + k * 3600
        rows.append({
            "id": str(k + 100), "epg_id": f"ch{stream_id}.synthetic.example",
            "title": b64(f"Mock show {stream_id}-{k}"), "description": b64("Invented program."),
            "start": time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(start)),
            "end": time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(start + 3600)),
            "start_timestamp": str(start), "stop_timestamp": str(start + 3600),
            "has_archive": 1 if table and k < 0 and stream_id % 5 == 0 else 0,
        })
    return {"epg_listings": rows}


def api(q):
    action = q.get("action")
    cat = q.get("category_id")
    if action in (None, "get_account_info", "get_profile"):
        return account()
    if action == "get_live_categories":
        return live_categories()
    if action == "get_live_streams":
        return live_streams(cat)
    if action == "get_vod_categories":
        return [{"category_id": str(100 + i), "category_name": f"Films {i}", "parent_id": 0} for i in range(4)]
    if action == "get_vod_streams":
        return vod_streams(cat)
    if action == "get_series_categories":
        return [{"category_id": str(200 + i), "category_name": f"Serials {i}", "parent_id": 0} for i in range(3)]
    if action == "get_series":
        return series_list(cat)
    if action == "get_vod_info":
        vid = int(q.get("vod_id", "0") or 0)
        return {
            "info": {"name": f"Mock Film {vid - 10000:03d}", "plot": "An invented film.", "duration_secs": 5400,
                     "genre": "Synthetic", "releasedate": "2020-01-01", "rating": "6.5"},
            "movie_data": {"stream_id": vid, "name": f"Mock Film {vid - 10000:03d}", "container_extension": "mp4"},
        }
    if action == "get_series_info":
        sid = int(q.get("series_id", "0") or 0)
        episodes = {
            str(s): [
                {"id": str(sid * 100 + s * 10 + e), "episode_num": e, "title": f"S{s}E{e}",
                 "container_extension": "mp4", "season": s, "info": {"plot": "Invented episode."}}
                for e in range(1, 5)
            ]
            for s in (1, 2)
        }
        return {"seasons": [{"season_number": 1, "name": "Season 1"}, {"season_number": 2, "name": "Season 2"}],
                "info": {"name": f"Mock Serial {sid - 20000:03d}", "plot": "An invented serial."},
                "episodes": episodes}
    if action == "get_short_epg":
        return short_epg(int(q.get("stream_id", "1") or 1), int(q.get("limit", "4") or 4))
    if action in ("get_simple_data_table", "get_simple_date_table"):
        return short_epg(int(q.get("stream_id", "1") or 1), 12, table=True)
    return []


def m3u_plus():
    lines = [f'#EXTM3U url-tvg="http://{ARGS.public}/xmltv.php?username={ARGS.user}&password={ARGS.password}"']
    for s in live_streams():
        lines.append(
            f'#EXTINF:-1 tvg-id="{s["epg_channel_id"]}" tvg-name="{s["name"]}" '
            f'group-title="{CATEGORIES[int(s["category_id"]) - 1]}",{s["name"]}'
        )
        lines.append(f'http://{ARGS.public}/live/{ARGS.user}/{ARGS.password}/{s["stream_id"]}.m3u8')
    return "\n".join(lines) + "\n"


def small_guide():
    now = int(time.time()) // 3600 * 3600
    parts = ['<?xml version="1.0" encoding="UTF-8"?>\n<tv>']
    for s in live_streams():
        parts.append(f'<channel id="{s["epg_channel_id"]}"><display-name>{s["name"]}</display-name></channel>')
    fmt = "%Y%m%d%H%M%S +0000"
    for s in live_streams():
        for k in range(-24, 48):
            a, b = now + k * 3600, now + (k + 1) * 3600
            parts.append(
                f'<programme start="{time.strftime(fmt, time.gmtime(a))}" stop="{time.strftime(fmt, time.gmtime(b))}" '
                f'channel="{s["epg_channel_id"]}"><title>Mock show {s["stream_id"]}-{k}</title></programme>'
            )
    parts.append("</tv>\n")
    return gzip.compress("\n".join(parts).encode())


class Handler(http.server.BaseHTTPRequestHandler):
    def send(self, status, body=b"", ctype="application/json", headers=()):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def redirect(self, n):
        self.send(302, headers=[("Location", f"{ARGS.hls_base}/ch{n % 3}/index.m3u8")])

    def authorized(self, user, password):
        return user == ARGS.user and password == ARGS.password

    def do_GET(self):
        url = urllib.parse.urlsplit(self.path)
        q = {k: v[0] for k, v in urllib.parse.parse_qs(url.query).items()}
        parts = [p for p in url.path.split("/") if p]
        if url.path.endswith("/player_api.php") or url.path.endswith("/panel_api.php"):
            if not self.authorized(q.get("username"), q.get("password")):
                return self.send(200, b'{"user_info":{"auth":0}}')
            return self.send(200, json.dumps(api(q)).encode())
        if url.path.endswith("/get.php"):
            if not self.authorized(q.get("username"), q.get("password")):
                return self.send(403, b"")
            return self.send(200, m3u_plus().encode(), "audio/x-mpegurl")
        if url.path.endswith("/xmltv.php"):
            if not self.authorized(q.get("username"), q.get("password")):
                return self.send(403, b"")
            path = os.path.join(ROOT, "fixtures", "hls", "iptv", "guide.xml.gz")
            body = open(path, "rb").read() if os.path.exists(path) else small_guide()
            return self.send(200, body, "application/gzip")
        # /live|movie|series/U/P/ID.ext and /timeshift/U/P/MIN/START/ID.ext
        if len(parts) >= 4 and parts[0] in ("live", "movie", "series", "timeshift"):
            if not self.authorized(parts[1], parts[2]):
                return self.send(403, b"")
            stem = parts[-1].split(".")[0]
            return self.redirect(int(stem) if stem.isdigit() else 0)
        if url.path.endswith("/streaming/timeshift.php"):
            if not self.authorized(q.get("username"), q.get("password")):
                return self.send(403, b"")
            return self.redirect(int(q.get("stream", "0") or 0))
        return self.send(404, b"not found", "text/plain")

    do_HEAD = do_GET

    def log_message(self, fmt, *args):
        line = REDACT_QUERY.sub(r"\1=***", fmt % args)
        line = REDACT_PATH.sub(r"/\1/***/***/", line)
        sys.stdout.write("%s %s\n" % (self.log_date_time_string(), line))
        sys.stdout.flush()


def main():
    global ARGS
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--port", type=int, default=8090)
    p.add_argument("--bind", default="0.0.0.0")
    p.add_argument("--public", default=None, help="host:port clients use (default 10.0.2.2:<port>)")
    p.add_argument("--hls-base", default="http://10.0.2.2:8089", help="where tools/mock-server.py serves ch0..ch2")
    p.add_argument("--user", default="demo")
    p.add_argument("--password", default="demo")
    p.add_argument("--channels", type=int, default=200)
    p.add_argument("--vod", type=int, default=40)
    p.add_argument("--series", type=int, default=12)
    p.add_argument("--status", default="Active", choices=["Active", "Expired", "Disabled", "Banned"])
    ARGS = p.parse_args()
    ARGS.public = ARGS.public or f"10.0.2.2:{ARGS.port}"
    server = http.server.ThreadingHTTPServer((ARGS.bind, ARGS.port), Handler)
    print(f"mock Xtream panel on :{ARGS.port} (user {ARGS.user}), streams -> {ARGS.hls_base}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
