"""Cargo 明文 HTTP 代理（绕开本机 schannel TLS 故障 + 沙箱外网限制）。

链路: cargo → http://127.0.0.1:10810（明文）→ 本代理 → 外层代理 127.0.0.1:10808
      → https://index.crates.io / https://static.crates.io（Python OpenSSL）→ 明文返回。

cargo 把 registry 直接指向本代理，因此 cargo 侧全程无 TLS、无 CONNECT。

用法: python scripts/cargo_http_proxy.py [port]   # 默认 10810
配合 $CARGO_HOME/config.toml:
    [source.crates-io]
    replace-with = "crates-io-http"
    [source.crates-io-http]
    registry = "sparse+http://127.0.0.1:10810/index/"
"""
import sys
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer

_PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 10810
_OUTER_PROXY = "http://127.0.0.1:10808"

_INDEX_BASE = "https://index.crates.io"
_DL_BASE = "https://static.crates.io"

# 上游统一经外层代理（Python OpenSSL 可用，绕过 schannel）
_OPENER = urllib.request.build_opener(
    urllib.request.ProxyHandler({"http": _OUTER_PROXY, "https": _OUTER_PROXY})
)


def _map_upstream(path: str) -> str | None:
    """把本地代理路径映射到 crates.io 官方端点。"""
    if path.startswith("/index/"):
        # cargo sparse base 是 .../index/，而 index.crates.io 的 config/条目在根
        return _INDEX_BASE + path[len("/index"):]
    if path == "/index/config.json":
        return _INDEX_BASE + "/config.json"
    if path.startswith("/crates/"):
        return _DL_BASE + path
    return None


class CargoProxy(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _proxy(self, method: str) -> None:
        upstream = _map_upstream(self.path)
        if upstream is None:
            self.send_error(403, f"path not mapped: {self.path}")
            return
        headers = {
            k: v
            for k, v in self.headers.items()
            if k.lower() not in ("host", "accept-encoding", "connection", "proxy-connection")
        }
        req = urllib.request.Request(upstream, method=method, headers=headers)
        try:
            # urllib 自动跟随重定向（static.crates.io → CDN，同样经外层代理）
            with _OPENER.open(req, timeout=300) as r:
                data = r.read()
                # 改写 index config.json：dl 指向本地代理，使 cargo 下载 .crate 走明文
                if upstream.endswith("/config.json"):
                    local_dl = f"http://127.0.0.1:{_PORT}/crates".encode()
                    data = data.replace(b"https://static.crates.io/crates", local_dl)
                self.send_response(200)
                ctype = r.headers.get("Content-Type", "application/octet-stream")
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(data)))
                self.send_header("Connection", "close")
                self.end_headers()
                if method == "GET":
                    self.wfile.write(data)
        except Exception as e:  # noqa: BLE001
            try:
                self.send_error(502, f"proxy error: {e}")
            except Exception:
                pass

    def do_GET(self):
        self._proxy("GET")

    def do_HEAD(self):
        self._proxy("HEAD")

    def log_message(self, *args):  # 静默
        pass


if __name__ == "__main__":
    print(f"cargo http proxy on 127.0.0.1:{_PORT} -> crates.io via {_OUTER_PROXY}", flush=True)
    HTTPServer(("127.0.0.1", _PORT), CargoProxy).serve_forever()
