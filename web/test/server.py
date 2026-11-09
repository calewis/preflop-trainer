"""Static test server on 127.0.0.1. In legacy mode it serves the old app's files
instead, so the service worker takeover can be tested at one address."""
import http.server, pathlib, socketserver, threading

ROOT = pathlib.Path(__file__).resolve().parents[2]
LEGACY = ROOT / "web/test/legacy"


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(ROOT), **kwargs)

    def log_message(self, *_):
        pass

    def translate_path(self, path):
        clean = path.split("?")[0].split("#")[0]
        if clean == "/app/crash-worker.js":
            return str(LEGACY.parent / "crash-worker.js")
        if self.server.legacy:
            target = LEGACY / (clean.lstrip("/") or "index.html")
            if target.is_file():
                return str(target)
        return super().translate_path(path)

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    extensions_map = dict(http.server.SimpleHTTPRequestHandler.extensions_map, **{
        ".js": "text/javascript", ".wasm": "application/wasm",
        ".webmanifest": "application/manifest+json", ".svg": "image/svg+xml",
    })


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True
    legacy = False


def start(port):
    server = Server(("127.0.0.1", port), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server
