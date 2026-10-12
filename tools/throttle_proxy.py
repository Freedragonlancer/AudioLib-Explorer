"""Throttling HTTP proxy for audition-streaming measurements (read-only by design).

    python tools/throttle_proxy.py --listen 127.0.0.1:18790 --upstream 192.168.1.10:8790 \
        --mbps 5 --latency-ms 175 --connect-ms 350 --log proxy.jsonl

* Every response byte (server -> client) passes one shared token bucket of `--mbps`
  (0 = unthrottled), like a single access link.
* `--latency-ms` delays each request (one round trip); `--connect-ms` is added to the first
  request of every new client connection (TCP + TLS handshakes over a remote reverse
  proxy). Client connections are kept alive, so a client that reuses connections pays the
  handshake once; each request still gets its own upstream connection (LAN, negligible).
* Only GET and HEAD without a body are forwarded; anything else (PATCH, POST /plays, ...)
  is answered 403 locally and never reaches the server.
* The log has one line per request: start/end times, method, path, status, bytes, client
  connection id and whether it was new, and the number of requests active at its start.
  The bench summarises it (503 BUSY count, peak concurrency, new connections).
"""
import argparse
import asyncio
import itertools
import json
import time

active_requests = 0
connection_ids = itertools.count(1)


class Bucket:
    def __init__(self, bytes_per_second: float):
        self.rate = bytes_per_second
        self.burst = max(16384.0, bytes_per_second * 0.01)
        self.tokens = 0.0
        self.last = time.monotonic()
        self.lock = asyncio.Lock()

    async def take(self, amount: int) -> None:
        if self.rate <= 0:
            return
        async with self.lock:
            while True:
                now = time.monotonic()
                self.tokens = min(self.burst, self.tokens + (now - self.last) * self.rate)
                self.last = now
                if self.tokens >= amount:
                    self.tokens -= amount
                    return
                await asyncio.sleep((amount - self.tokens) / self.rate)


async def read_head(reader: asyncio.StreamReader) -> bytes:
    try:
        return await reader.readuntil(b"\r\n\r\n")
    except (asyncio.IncompleteReadError, asyncio.LimitOverrunError, ConnectionError):
        return b""


def without_hop_headers(lines):
    return [line for line in lines if line and not line.lower().startswith(("connection:", "keep-alive:"))]


async def forward(method, lines, headers, args, bucket, client_writer, record) -> bool:
    """One request upstream; returns whether the client connection can stay open."""
    host, port = args.upstream.rsplit(":", 1)
    upstream_reader, upstream_writer = await asyncio.open_connection(host, int(port))
    try:
        upstream_writer.write(("\r\n".join([lines[0], *without_hop_headers(headers), "Connection: close"]) + "\r\n\r\n").encode("latin-1"))
        await upstream_writer.drain()
        head = await read_head(upstream_reader)
        if not head:
            record["status"] = 0
            return False
        record["first_byte"] = time.time() - record["start"]
        response = head.decode("latin-1").split("\r\n")
        try:
            status = int(response[0].split(" ", 2)[1])
        except (IndexError, ValueError):
            status = 0
        record["status"] = status
        fields = [line.lower().replace(" ", "") for line in response[1:]]
        framed = (
            method == "HEAD"
            or status in (204, 304)
            or any(f.startswith("content-length:") for f in fields)
            or any(f.startswith("transfer-encoding:") and "chunked" in f for f in fields)
        )
        out = [response[0], *without_hop_headers(response[1:])]
        if not framed:
            out.append("Connection: close")
        data = ("\r\n".join(out) + "\r\n\r\n").encode("latin-1")
        await bucket.take(len(data))
        client_writer.write(data)
        await client_writer.drain()
        sent = 0
        while True:
            chunk = await upstream_reader.read(16384)
            if not chunk:
                break
            await bucket.take(len(chunk))
            client_writer.write(chunk)
            await client_writer.drain()
            sent += len(chunk)
        record["bytes"] = sent
        return framed
    finally:
        try:
            upstream_writer.close()
        except Exception:
            pass


async def handle(client_reader, client_writer, args, bucket, log):
    global active_requests
    connection = next(connection_ids)
    requests = 0
    try:
        while True:
            head = await read_head(client_reader)
            if not head:
                return
            requests += 1
            active_requests += 1
            record = {"start": time.time(), "concurrent": active_requests, "conn": connection, "new_conn": requests == 1}
            keep = False
            try:
                lines = head.decode("latin-1").split("\r\n")
                method, path = (lines[0].split(" ") + ["", ""])[:2]
                record.update(method=method, path=path)
                headers = [line for line in lines[1:] if line]
                body = any(
                    line.lower().startswith(("content-length:", "transfer-encoding:"))
                    and not line.lower().replace(" ", "").endswith(":0")
                    for line in headers
                )
                if method not in ("GET", "HEAD") or body:
                    record["status"] = 403
                    client_writer.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    await client_writer.drain()
                    return
                delay = args.latency_ms + (args.connect_ms if requests == 1 else 0)
                if delay:
                    await asyncio.sleep(delay / 1000)
                keep = await forward(method, lines, headers, args, bucket, client_writer, record)
                client_close = any(line.lower().replace(" ", "") == "connection:close" for line in headers)
                keep = keep and not client_close
            except (ConnectionError, OSError) as error:
                record["error"] = type(error).__name__
            finally:
                active_requests -= 1
                record["end"] = time.time()
                log.write(json.dumps(record) + "\n")
                log.flush()
            if not keep:
                return
    finally:
        try:
            client_writer.close()
        except Exception:
            pass


async def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--listen", default="127.0.0.1:18790")
    parser.add_argument("--upstream", required=True)
    parser.add_argument("--mbps", type=float, default=0.0)
    parser.add_argument("--latency-ms", type=float, default=0.0)
    parser.add_argument("--connect-ms", type=float, default=0.0)
    parser.add_argument("--log", required=True)
    args = parser.parse_args()
    bucket = Bucket(args.mbps * 1_000_000 / 8)
    log = open(args.log, "a", encoding="utf-8")
    host, port = args.listen.rsplit(":", 1)
    server = await asyncio.start_server(
        lambda r, w: handle(r, w, args, bucket, log), host, int(port), limit=65536
    )
    print(f"listening on {args.listen} -> {args.upstream} at {args.mbps or 'unlimited'} Mbps", flush=True)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
