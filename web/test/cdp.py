"""A minimal Chrome DevTools client over a raw WebSocket (standard library only)."""
import base64, hashlib, json, os, socket, struct, time, urllib.parse, urllib.request


class Socket:
    def __init__(self, url):
        parsed = urllib.parse.urlparse(url)
        self.sock = socket.create_connection((parsed.hostname, parsed.port))
        key = base64.b64encode(os.urandom(16)).decode()
        path = parsed.path + ("?" + parsed.query if parsed.query else "")
        self.sock.sendall((
            f"GET {path} HTTP/1.1\r\nHost: {parsed.hostname}:{parsed.port}\r\n"
            f"Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\nOrigin: http://127.0.0.1\r\n\r\n").encode())
        response = b""
        while b"\r\n\r\n" not in response:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise IOError("websocket handshake failed")
            response += chunk
        head, self.buffer = response.split(b"\r\n\r\n", 1)
        if b"101" not in head.split(b"\r\n")[0]:
            raise IOError(head.decode(errors="replace"))

    def send(self, text):
        data = text.encode()
        header = bytearray([0x81])
        n = len(data)
        if n < 126:
            header.append(0x80 | n)
        elif n < 65536:
            header.append(0x80 | 126)
            header += struct.pack(">H", n)
        else:
            header.append(0x80 | 127)
            header += struct.pack(">Q", n)
        mask = os.urandom(4)
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
        self.sock.sendall(bytes(header) + mask + masked)

    def _fill(self, n):
        while len(self.buffer) < n:
            chunk = self.sock.recv(1 << 16)
            if not chunk:
                raise IOError("websocket closed")
            self.buffer += chunk

    def recv(self):
        """One text message. Nothing is consumed until a whole frame has arrived, so a
        socket timeout can be retried without losing bytes."""
        message = b""
        while True:
            self._fill(2)
            head = self.buffer[:2]
            opcode, length, offset = head[0] & 0x0f, head[1] & 0x7f, 2
            if length == 126:
                self._fill(4)
                length, offset = struct.unpack(">H", self.buffer[2:4])[0], 4
            elif length == 127:
                self._fill(10)
                length, offset = struct.unpack(">Q", self.buffer[2:10])[0], 10
            self._fill(offset + length)
            payload = self.buffer[offset:offset + length]
            self.buffer = self.buffer[offset + length:]
            if opcode == 8:
                raise IOError("websocket closed by peer")
            if opcode in (9, 10):
                continue
            message += payload
            if head[0] & 0x80:
                return message.decode()


class Chrome:
    def __init__(self, port):
        self.port = port
        self.errors = []

    def new_page(self, url="about:blank"):
        req = urllib.request.Request(f"http://127.0.0.1:{self.port}/json/new?{urllib.parse.quote(url)}", method="PUT")
        info = json.load(urllib.request.urlopen(req))
        self.page = Page(info["webSocketDebuggerUrl"], self.errors)
        return self.page


class Page:
    def __init__(self, ws_url, errors):
        self.sock = Socket(ws_url)
        self.next_id = 1
        self.errors = errors
        self.event_counts = {}
        self.send("Runtime.enable")
        self.send("Log.enable")
        self.send("Page.enable")

    def send(self, method, params=None, timeout=None):
        self.next_id += 1
        my_id = self.next_id
        self.sock.send(json.dumps({"id": my_id, "method": method, "params": params or {}}))
        self.sock.sock.settimeout(timeout)
        deadline = time.time() + timeout if timeout else None
        while True:
            if deadline and time.time() > deadline:
                raise socket.timeout("no reply in time")
            message = json.loads(self.sock.recv())
            if message.get("id") == my_id:
                if "error" in message:
                    raise RuntimeError(f"{method}: {message['error']}")
                return message.get("result", {})
            self.on_event(message)

    def on_event(self, message):
        method = message.get("method")
        self.event_counts[method] = self.event_counts.get(method, 0) + 1
        if method == "Runtime.exceptionThrown":
            d = message["params"]["exceptionDetails"]
            text = d.get("exception", {}).get("description") or d.get("text")
            self.errors.append(f"uncaught: {text}")
        elif method == "Log.entryAdded":
            entry = message["params"]["entry"]
            if entry["level"] == "error" and "favicon" not in entry.get("url", ""):
                self.errors.append(f"console error: {entry['text']} {entry.get('url','')}")

    def eval(self, expression, timeout=None):
        result = self.send("Runtime.evaluate", {
            "expression": expression, "awaitPromise": True, "returnByValue": True, "userGesture": True,
        }, timeout)
        if result.get("exceptionDetails"):
            raise RuntimeError(f"eval failed: {expression[:200]}\n{json.dumps(result['exceptionDetails'])[:800]}")
        return result["result"].get("value")

    def wait(self, expression, timeout=30, interval=0.02):
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                value = self.eval(expression, timeout=5)
            except socket.timeout:
                value = None  # the page was mid-navigation; ask again
            except RuntimeError as err:
                if "navigated" not in str(err) and "context" not in str(err).lower():
                    raise
                value = None
            if value:
                return value
            time.sleep(interval)
        raise TimeoutError(f"timed out waiting for: {expression[:300]}")

    def navigate(self, url):
        self.send("Page.navigate", {"url": url})

    def reload(self):
        self.send("Page.reload", {"ignoreCache": False})

    def screenshot(self, path):
        data = self.send("Page.captureScreenshot", {"format": "png"})["data"]
        with open(path, "wb") as f:
            f.write(base64.b64decode(data))

    def viewport(self, width, height, mobile):
        self.send("Emulation.setDeviceMetricsOverride", {
            "width": width, "height": height, "deviceScaleFactor": 2 if mobile else 1, "mobile": mobile})
        self.send("Emulation.setTouchEmulationEnabled", {"enabled": mobile})

    def theme(self, scheme):
        self.send("Emulation.setEmulatedMedia", {"features": [{"name": "prefers-color-scheme", "value": scheme}]})
