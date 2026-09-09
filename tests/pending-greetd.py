"""Controlled fake greetd peer for the isolated preview; synthetic credentials only."""

import json
from pathlib import Path
import socket
import struct
import sys
import time

output = Path(sys.argv[1])


def wait_for(name):
    deadline = time.monotonic() + 15
    while not (output / name).exists():
        if time.monotonic() >= deadline:
            raise TimeoutError(name)
        time.sleep(0.05)


with socket.socket(socket.AF_UNIX) as server:
    server.bind(str(output / "greetd.sock"))
    server.listen(1)
    (output / "server-ready").touch()
    with server.accept()[0] as connection:
        stream = connection.makefile("rwb", buffering=0)

        def receive(expected):
            size = struct.unpack("=I", stream.read(4))[0]
            assert json.loads(stream.read(size)) == expected

        def send(response):
            data = json.dumps(response, ensure_ascii=False).encode()
            stream.write(struct.pack("=I", len(data)) + data)

        receive({"type": "create_session", "username": "demo-user"})
        (output / "create-received").touch()
        wait_for("release-create")
        send({"type": "auth_message", "auth_message_type": "secret", "auth_message": "Password:"})
        receive({"type": "post_auth_message_response", "response": "demo-password"})
        (output / "password-received").touch()
        wait_for("release-password")
        send({"type": "error", "error_type": "auth_error", "description": "Test rejection"})
        receive({"type": "cancel_session"})
        (output / "cancel-received").touch()
        wait_for("release-cancel")
        send({"type": "success"})
        (output / "server-passed").touch()
