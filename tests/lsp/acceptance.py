#!/usr/bin/env python3
"""Stdlib-only acceptance probe for Lean 4.34.0's LSP and widget RPC seam.

This is intentionally a small black-box client.  It reports observed shapes and
does not claim that an editor host implements any of these operations.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
from typing import Any


class LspError(RuntimeError):
    pass


class Client:
    def __init__(self, root: pathlib.Path) -> None:
        self.root = root
        self.proc = subprocess.Popen(
            ["elan", "run", "v4.34.0", "lean", "--server"],
            cwd=root,
            env={**os.environ, "LEAN_PATH": str(root) + os.pathsep + os.environ.get("LEAN_PATH", "")},
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=False,
        )
        self.next_id = 1
        self.pending: dict[Any, dict[str, Any]] = {}
        self.notifications: list[dict[str, Any]] = []
        self.stderr = bytearray()
        self._stderr_thread = threading.Thread(target=self._drain_stderr, daemon=True)
        self._stderr_thread.start()

    def _drain_stderr(self) -> None:
        assert self.proc.stderr is not None
        while True:
            chunk = self.proc.stderr.read(4096)
            if not chunk:
                return
            self.stderr.extend(chunk)

    def close(self) -> None:
        if self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait(timeout=5)

    def send(self, message: dict[str, Any]) -> None:
        raw = json.dumps(message, separators=(",", ":")).encode()
        assert self.proc.stdin is not None
        self.proc.stdin.write(f"Content-Length: {len(raw)}\r\n\r\n".encode() + raw)
        self.proc.stdin.flush()

    def notify(self, method: str, params: dict[str, Any] | None = None) -> None:
        msg: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        self.send(msg)

    def _read_message(self, timeout: float = 20.0) -> dict[str, Any]:
        assert self.proc.stdout is not None
        deadline = time.monotonic() + timeout
        headers: dict[str, str] = {}
        while True:
            if time.monotonic() > deadline:
                raise TimeoutError("timed out waiting for LSP headers")
            line = self.proc.stdout.readline()
            if not line:
                raise LspError(f"Lean server exited ({self.proc.returncode}); stderr={self.stderr.decode(errors='replace')}")
            line = line.decode("ascii", errors="replace").rstrip("\r\n")
            if not line:
                break
            key, sep, value = line.partition(":")
            if sep:
                headers[key.lower()] = value.strip()
        try:
            length = int(headers["content-length"])
        except (KeyError, ValueError) as exc:
            raise LspError(f"bad LSP headers: {headers}") from exc
        body = self.proc.stdout.read(length)
        if len(body) != length:
            raise LspError("truncated LSP body")
        return json.loads(body)

    def request(self, method: str, params: dict[str, Any] | None = None, timeout: float = 30.0) -> Any:
        ident = self.next_id
        self.next_id += 1
        msg: dict[str, Any] = {"jsonrpc": "2.0", "id": ident, "method": method}
        if params is not None:
            msg["params"] = params
        self.send(msg)
        deadline = time.monotonic() + timeout
        while True:
            remaining = max(0.1, deadline - time.monotonic())
            response = self._read_message(remaining)
            if "method" in response:
                self.notifications.append(response)
                # Lean may ask the client to create progress tokens or register a
                # capability.  A null response is enough for this probe.
                if "id" in response:
                    self.send({"jsonrpc": "2.0", "id": response["id"], "result": None})
                continue
            if response.get("id") != ident:
                self.pending[response.get("id")] = response
                continue
            if "error" in response:
                raise LspError(f"{method} failed: {json.dumps(response['error'], sort_keys=True)}")
            return response.get("result")

    def shutdown(self) -> None:
        try:
            self.request("shutdown", {}, timeout=10)
            self.notify("exit")
            self.proc.wait(timeout=10)
        finally:
            self.close()


def file_uri(path: pathlib.Path) -> str:
    return urllib.parse.urlunparse(("file", "", str(path), "", "", ""))


def pos(line: int, character: int) -> dict[str, int]:
    return {"line": line, "character": character}


def rpc_refs(value: Any) -> list[dict[str, Any]]:
    """Collect opaque refs from either Lean RPC wire format."""
    found: list[dict[str, Any]] = []
    if isinstance(value, dict):
        if set(value) == {"p"} or set(value) == {"__rpcref"}:
            found.append(value)
        else:
            for child in value.values():
                found.extend(rpc_refs(child))
    elif isinstance(value, list):
        for child in value:
            found.extend(rpc_refs(child))
    return found


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--json", action="store_true", help="emit the sanitized report as JSON")
    args = parser.parse_args()
    fixture = pathlib.Path(__file__).with_name("fixtures") / "acceptance.lean"
    with tempfile.TemporaryDirectory(prefix="lean4-hx-lsp-") as temporary:
        root = pathlib.Path(temporary)
        source_path = root / "Acceptance.lean"
        helper_path = root / "Helper.lean"
        source = fixture.read_text(encoding="utf-8")
        helper_source = (fixture.parent / "Helper.lean").read_text(encoding="utf-8")
        source_path.write_text(source, encoding="utf-8")
        helper_path.write_text(helper_source, encoding="utf-8")
        subprocess.run(
            ["elan", "run", "v4.34.0", "lean", "-R", str(root), "-o", str(root / "Helper.olean"), str(helper_path)],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        uri = file_uri(source_path)
        c = Client(root)
        report: dict[str, Any] = {"toolchain": "v4.34.0", "server": "lean --server", "checks": {}}
        try:
            init = c.request(
                "initialize",
                {
                    "processId": None,
                    "rootUri": file_uri(root),
                    "capabilities": {
                        "textDocument": {
                            "completion": {"completionItem": {"snippetSupport": False}},
                            "codeAction": {"codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["quickfix", "refactor", "source"]}}},
                            "inlayHint": {"resolveSupport": {"properties": []}},
                        },
                        "workspace": {"workspaceFolders": True},
                    },
                    "workspaceFolders": [{"uri": file_uri(root), "name": "acceptance"}],
                },
            )
            c.notify("initialized", {})
            report["initialize"] = {
                "serverInfo": init.get("serverInfo"),
                "capabilities": sorted(k for k, v in init.get("capabilities", {}).items() if v),
            }
            helper_uri = file_uri(helper_path)
            c.notify("textDocument/didOpen", {"textDocument": {"uri": helper_uri, "languageId": "lean", "version": 1, "text": helper_source}})
            c.request("textDocument/waitForDiagnostics", {"uri": helper_uri, "version": 1}, timeout=30)
            c.notify("textDocument/didOpen", {"textDocument": {"uri": uri, "languageId": "lean", "version": 1, "text": source}})
            c.request("textDocument/waitForDiagnostics", {"uri": uri, "version": 1}, timeout=30)
            diagnostics = [n for n in c.notifications if n.get("method") == "textDocument/publishDiagnostics"]
            progress = [n for n in c.notifications if n.get("method") in {"$/lean/fileProgress", "$ /lean/fileProgress", "$/lean/processing"}]
            report["checks"]["didOpen_diagnostics_progress"] = {
                "diagnosticNotifications": len(diagnostics),
                "diagnosticCount": sum(len(n.get("params", {}).get("diagnostics", [])) for n in diagnostics),
                "diagnosticMessages": [
                    d.get("message", "").replace(str(root), "<fixture-root>").replace(str(pathlib.Path.home()), "<user>")
                    for n in diagnostics for d in n.get("params", {}).get("diagnostics", [])
                ],
                "diagnosticMethods": sorted({n.get("method") for n in diagnostics}),
                "progressNotifications": len(progress),
                "progressMethods": sorted({n.get("method") for n in progress}),
                "passed": bool(diagnostics) and bool(progress),
            }
            text_document = {"textDocument": {"uri": uri}}
            position = {"line": 5, "character": 2}  # the `exact` tactic
            goal_params = {**text_document, "position": position}
            plain_goal = c.request("$/lean/plainGoal", goal_params)
            term_goal = c.request("$/lean/plainTermGoal", {**text_document, "position": pos(2, 20)})
            report["checks"]["plainGoal_plainTermGoal"] = {
                "plainGoalPresent": plain_goal is not None,
                "plainGoalKeys": sorted(plain_goal) if isinstance(plain_goal, dict) else [],
                "plainTermGoalPresent": term_goal is not None,
                "plainTermGoalKeys": sorted(term_goal) if isinstance(term_goal, dict) else [],
                "plainGoalGoals": len(plain_goal.get("goals", [])) if isinstance(plain_goal, dict) else 0,
                "passed": isinstance(plain_goal, dict) and isinstance(term_goal, dict),
            }
            baseline: dict[str, Any] = {}
            baseline["hover"] = c.request("textDocument/hover", {**text_document, "position": pos(2, 4)})
            baseline["completion"] = c.request("textDocument/completion", {**text_document, "position": pos(2, 6)})
            baseline["definition"] = c.request("textDocument/definition", {**text_document, "position": pos(2, 4)})
            baseline["crossFileDefinition"] = c.request("textDocument/definition", {**text_document, "position": pos(2, 20)})
            baseline["references"] = c.request("textDocument/references", {**text_document, "position": pos(2, 4), "context": {"includeDeclaration": True}})
            baseline["codeAction"] = c.request("textDocument/codeAction", {**text_document, "range": {"start": pos(7, 7), "end": pos(7, 20)}, "context": {"diagnostics": diagnostics[-1].get("params", {}).get("diagnostics", []) if diagnostics else []}})
            baseline["inlayHint"] = c.request("textDocument/inlayHint", {**text_document, "range": {"start": pos(0, 0), "end": pos(6, 15)}})
            report["checks"]["ordinary_lsp_baseline"] = {
                "hover": isinstance(baseline["hover"], dict),
                "completion": isinstance(baseline["completion"], dict),
                "completionItems": len(baseline["completion"].get("items", [])) if isinstance(baseline["completion"], dict) else 0,
                "definition": isinstance(baseline["definition"], list),
                "definitionCrossFile": any(
                    isinstance(item, dict)
                    and "Helper.lean" in str(item.get("targetUri", item.get("uri", "")))
                    for item in (baseline["crossFileDefinition"] if isinstance(baseline["crossFileDefinition"], list) else [])
                ),
                "definitionItems": [
                    {k: (v.replace(str(root), "<fixture-root>") if isinstance(v, str) else v)
                     for k, v in item.items() if k in {"targetUri", "uri", "targetRange", "range"}}
                    for item in (baseline["definition"] if isinstance(baseline["definition"], list) else [])
                    if isinstance(item, dict)
                ],
                "crossFileDefinitionItems": [
                    {k: (v.replace(str(root), "<fixture-root>") if isinstance(v, str) else v)
                     for k, v in item.items() if k in {"targetUri", "uri", "targetRange", "range"}}
                    for item in (baseline["crossFileDefinition"] if isinstance(baseline["crossFileDefinition"], list) else [])
                    if isinstance(item, dict)
                ],
                "references": isinstance(baseline["references"], list),
                "codeAction": isinstance(baseline["codeAction"], list),
                "inlayHint": isinstance(baseline["inlayHint"], list),
                "unsupported": [k for k, v in baseline.items() if v is None],
            }
            report["checks"]["ordinary_lsp_baseline"]["passed"] = all(
                report["checks"]["ordinary_lsp_baseline"][key]
                for key in ("hover", "completion", "definition", "references", "codeAction", "inlayHint")
            )
            connect = c.request("$/lean/rpc/connect", {"uri": uri})
            session_id = connect.get("sessionId") if isinstance(connect, dict) else None
            rpc_params = {"textDocument": {"uri": uri}, "position": position, "sessionId": session_id, "method": "Lean.Widget.getInteractiveGoals", "params": goal_params}
            rpc_result = c.request("$/lean/rpc/call", rpc_params)
            refs = rpc_refs(rpc_result)
            c.notify("$/lean/rpc/release", {"uri": uri, "sessionId": session_id, "refs": refs})
            report["checks"]["rpc_connect_call_release"] = {
                "connected": isinstance(connect, dict) and "sessionId" in connect,
                "method": "Lean.Widget.getInteractiveGoals",
                "resultPresent": rpc_result is not None,
                "resultKeys": sorted(rpc_result) if isinstance(rpc_result, dict) else [],
                "refsReleased": len(refs),
                "releaseIssued": True,
                "passed": isinstance(connect, dict) and "sessionId" in connect and rpc_result is not None,
            }
            report["passed"] = all(v.get("passed", True) for v in report["checks"].values())
        finally:
            c.shutdown()
        # No absolute or user-specific paths are included in the report.
        if args.json:
            print(json.dumps(report, sort_keys=True))
        else:
            print(json.dumps(report, sort_keys=True, indent=2))
    return 0 if report.get("passed") else 1


if __name__ == "__main__":
    sys.exit(main())
