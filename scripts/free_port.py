#!/usr/bin/env python3
"""Select an unused loopback TCP port for a local test service."""
import socket

if __name__ == "__main__":
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        print(sock.getsockname()[1])
