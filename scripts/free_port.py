#!/usr/bin/env python3
"""Print one free TCP port on 127.0.0.1. Stdlib only (T-1101a).

The port is chosen by binding :0 and released immediately, so a caller can hand
it to a service that binds it a moment later. Good enough for a single-run local
stack; every service is bound to loopback so parallel runs on one host do not
collide on fixed ports.
"""

import socket


def main() -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        print(sock.getsockname()[1])


if __name__ == "__main__":
    main()
