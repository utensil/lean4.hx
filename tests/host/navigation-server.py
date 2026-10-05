#!/usr/bin/env python3
"""Deterministic definition server for the editor-navigation acceptance."""

from __future__ import annotations

import json
import os
import pathlib
import sys

ROOT = pathlib.Path(os.environ["LEAN4_HX_NAVIGATION_ROOT"]).resolve()
MAIN_URI = (ROOT / "Main.lean").as_uri()
TARGET_NAME = os.environ.get("LEAN4_HX_NAVIGATION_TARGET", "Main.lean")
TARGET_URI = (ROOT / TARGET_NAME).as_uri()
START_CHARACTER = int(os.environ.get("LEAN4_HX_NAVIGATION_START_CHARACTER", "0"))
END_CHARACTER = int(os.environ.get("LEAN4_HX_NAVIGATION_END_CHARACTER", "5"))

def send(message: dict) -> None:
    body = json.dumps(message, separators=(",", ":")).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode())
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

def receive() -> dict | None:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.rstrip(b"\r\n")
        if not line:
            break
        name, value = line.split(b":", 1)
        if name.lower() == b"content-length":
            length = int(value.strip())
    if length is None:
        raise RuntimeError("missing Content-Length")
    return json.loads(sys.stdin.buffer.read(length))

while True:
    message = receive()
    if message is None:
        break
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"capabilities": {"definitionProvider": True, "textDocumentSync": 1}}})
    elif method == "textDocument/definition":
        send({"jsonrpc": "2.0", "id": request_id, "result": [{"uri": TARGET_URI, "range": {"start": {"line": 0, "character": START_CHARACTER}, "end": {"line": 0, "character": END_CHARACTER}}}]})
    elif request_id is not None:
        send({"jsonrpc": "2.0", "id": request_id, "result": None})
