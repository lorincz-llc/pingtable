#!/usr/bin/env python3
"""A TCP service that keeps restarting, for the website demo recording.

Listens on 127.0.0.1:PORT for --up seconds, closes the port for --down
seconds, and repeats until stopped. Connections are accepted and closed at
once; nothing is read or sent. While it is closed, a connection is refused,
so pingtable shows the row going down and coming back.

    scripts/demo-flappy.py --port 8180 --up 55 --down 20

scripts/record-demo.py starts and stops it by itself.
"""

import argparse
import select
import socket
import sys
import time


def serve_for(port, seconds):
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(("127.0.0.1", port))
    sock.listen(16)
    end = time.monotonic() + seconds
    try:
        while True:
            left = end - time.monotonic()
            if left <= 0:
                return
            ready, _, _ = select.select([sock], [], [], min(left, 0.5))
            if ready:
                conn, _ = sock.accept()
                conn.close()
    finally:
        sock.close()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", type=int, default=8180)
    ap.add_argument("--up", type=float, default=55, help="seconds listening")
    ap.add_argument("--down", type=float, default=20, help="seconds closed")
    args = ap.parse_args()
    try:
        while True:
            print(f"{time.strftime('%H:%M:%S')} listening on 127.0.0.1:{args.port}", flush=True)
            serve_for(args.port, args.up)
            print(f"{time.strftime('%H:%M:%S')} closed", flush=True)
            time.sleep(args.down)
    except KeyboardInterrupt:
        return 0


if __name__ == "__main__":
    sys.exit(main())
