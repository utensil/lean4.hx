#!/usr/bin/env python3
"""Tiny deterministic LSP server for the two-file WorkspaceEdit acceptance."""

import json
import os
import pathlib
import sys
from typing import Optional


ROOT = pathlib.Path(os.environ["LEAN4_HX_WORKSPACE_ROOT"])
MAIN = ROOT / "Main.lean"
SECOND = ROOT / "Second.lean"


def uri(path: pathlib.Path) -> str:
    return path.resolve().as_uri()


def send(message: dict) -> None:
    body = json.dumps(message, separators=(",", ":")).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode())
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()


def receive() -> Optional[dict]:
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


def valid_edit() -> dict:
    return {
        "title": "Apply two-file fixture",
        "kind": "quickfix",
        "edit": {
            "changes": {
                uri(MAIN): [
                    {
                        "range": {
                            "start": {"line": 1, "character": 2},
                            "end": {"line": 1, "character": 5},
                        },
                        "newText": "exact rfl",
                    }
                ],
                uri(SECOND): [
                    {
                        "range": {
                            "start": {"line": 0, "character": 20},
                            "end": {"line": 0, "character": 21},
                        },
                        "newText": "1",
                    }
                ],
            }
        },
    }


def stale_edit() -> dict:
    return {
        "title": "Reject stale two-file fixture",
        "kind": "quickfix",
        "edit": {
            "documentChanges": [
                {
                    "textDocument": {"uri": uri(MAIN), "version": 0},
                    "edits": [
                        {
                            "range": {
                                "start": {"line": 1, "character": 2},
                                "end": {"line": 1, "character": 11},
                            },
                            "newText": "rfl",
                        }
                    ],
                },
                {
                    "textDocument": {"uri": uri(SECOND), "version": 0},
                    "edits": [
                        {
                            "range": {
                                "start": {"line": 0, "character": 20},
                                "end": {"line": 0, "character": 21},
                            },
                            "newText": "2",
                        }
                    ],
                },
            ]
        },
    }


while True:
    message = receive()
    if message is None:
        break
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "capabilities": {
                        "codeActionProvider": {"codeActionKinds": ["quickfix"]},
                        "textDocumentSync": 1,
                    }
                },
            }
        )
    elif method == "textDocument/codeAction":
        send({"jsonrpc": "2.0", "id": request_id, "result": [valid_edit(), stale_edit()]})
    elif request_id is not None:
        send({"jsonrpc": "2.0", "id": request_id, "result": None})
