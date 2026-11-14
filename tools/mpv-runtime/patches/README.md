# Native relay capability

These local FFmpeg changes are retained with the corresponding source archive
and included in each runtime's recipe digest. They are independent of the HLS
URI rewriter.

A client opts a stream into the guard using `patina_http_prefix` in both
`stream-lavf-o` and `demuxer-lavf-o`. It must check the advertised
`patina-relay-guard-v1` protocol first and keep `demuxer-lavf-propagate-opts`
enabled. The marker protocol cannot open a resource.

The format context keeps the prefix for nested opens, including callers with
no option dictionary. URL contexts inherit it into encrypted segments. Guarded
IO permits the local HTTP capability and its internal TCP transport; other
protocols and addresses fail before connecting. HTTP checks every redirect and
reconnect before opening a transport. Guarded local HTTP bypasses inherited
proxies without changing the process environment.

Desktop builds explicitly disable mpv's libcurl backend and validate that
configuration: guarded IO must use this patched Lavf implementation. iOS must
also be validated with its rebuilt, pinned native library before enabling the
guard there.

`relay_guard.py` serves only generated loopback fixtures and passes raw HLS to
the native player, deliberately bypassing the rewriter. It checks ordinary and
AES playback, refused foreign keys/segments, redirects, foreign protocols and
other sessions under several inherited proxy environments. Forbidden and proxy
servers must accept zero connections. `--baseline` reproduces the old native
bypass using a stock runtime; it is not a passing guard check.
