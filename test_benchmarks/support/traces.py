"""Perfetto trace composition without workflow-specific knowledge."""

import http.server
import json
from pathlib import Path
import socketserver
import time
from functools import partial
from typing import Callable, Iterable
import urllib.parse
import webbrowser


PERFETTO_ORIGIN = "https://ui.perfetto.dev"
PERFETTO_PORT = 9001
PERFETTO_REQUEST_TIMEOUT_SECONDS = 30


class _TraceServer(socketserver.TCPServer):
    allow_reuse_address = True


class _TraceHandler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self) -> None:
        self.send_header("Access-Control-Allow-Origin", PERFETTO_ORIGIN)
        self.send_header("Access-Control-Allow-Methods", "GET, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "*")
        self.send_header("Cache-Control", "no-cache")
        super().end_headers()

    def do_OPTIONS(self) -> None:
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self) -> None:
        requested_path = urllib.parse.urlsplit(self.path).path
        if requested_path != f"/{self.server.trace_name}":
            self.send_error(404, "File not found")
            return
        self.server.trace_requested = True
        super().do_GET()

    def log_message(self, format: str, *args: object) -> None:
        del format, args


def combine_traces(inputs: Iterable[Path], output: Path) -> None:
    events: list[dict] = []
    pid_offset = 0
    tid_offset = 0
    timestamp_offset = 0.0
    for source in inputs:
        if not source.exists():
            continue
        document = json.loads(source.read_text(encoding="utf-8"))
        local_events = document.get("traceEvents", [])
        if not local_events:
            continue
        pids = [event.get("pid", 0) for event in local_events if isinstance(event.get("pid", 0), int)]
        tids = [event.get("tid", 0) for event in local_events if isinstance(event.get("tid", 0), int)]
        minimum_ts = min((event.get("ts", 0.0) for event in local_events), default=0.0)
        for event in local_events:
            copied = dict(event)
            if isinstance(copied.get("pid"), int):
                copied["pid"] += pid_offset
            if isinstance(copied.get("tid"), int):
                copied["tid"] += tid_offset
            if isinstance(copied.get("ts"), (int, float)):
                copied["ts"] = copied["ts"] - minimum_ts + timestamp_offset
            events.append(copied)
        pid_offset += max(pids, default=0) + 1
        tid_offset += max(tids, default=0) + 1
        timestamp_offset = max((event.get("ts", 0.0) for event in events), default=0.0) + 1.0
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps({"traceEvents": events, "displayTimeUnit": "ms"}, indent=2) + "\n",
        encoding="utf-8",
    )


def combine_labeled_traces(inputs: Iterable[tuple[str, Path]], output: Path) -> None:
    """Combine phase traces while keeping every benchmark lane in one process."""
    events: list[dict] = []
    lane_process_ids: dict[str, int] = {}
    lane_thread_ids: dict[str, dict[object, int]] = {}
    lane_offsets: dict[str, int] = {}

    for label, source in inputs:
        if not source.is_file():
            continue
        document = json.loads(source.read_text(encoding="utf-8"))
        local_events = [
            event
            for event in document.get("traceEvents", [])
            if isinstance(event, dict)
        ]
        if not local_events:
            continue
        if label not in lane_process_ids:
            lane_process_ids[label] = len(lane_process_ids) + 1
            lane_thread_ids[label] = {}
            lane_offsets[label] = 0
            events.append(
                {
                    "name": "process_name",
                    "cat": "__metadata",
                    "ph": "M",
                    "ts": 0,
                    "pid": lane_process_ids[label],
                    "tid": 1,
                    "args": {"name": label},
                }
            )

        process_id = lane_process_ids[label]
        thread_ids = lane_thread_ids[label]
        offset = lane_offsets[label]
        source_duration = 0
        for source_event in local_events:
            if source_event.get("cat") == "__metadata" and source_event.get("name") == "process_name":
                continue
            event = dict(source_event)
            source_thread_id = event.get("tid", 0)
            if source_thread_id not in thread_ids:
                thread_ids[source_thread_id] = len(thread_ids) + 1
            event["pid"] = process_id
            event["tid"] = thread_ids[source_thread_id]
            source_timestamp = event.get("ts", 0)
            if isinstance(source_timestamp, (int, float)):
                source_duration = max(source_duration, int(source_timestamp))
                event["ts"] = int(source_timestamp) + offset
            events.append(event)
        lane_offsets[label] = offset + source_duration + 1

    events.sort(key=lambda event: (event.get("ts", 0), event.get("pid", 0)))
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps({"traceEvents": events, "displayTimeUnit": "ms"}, indent=2) + "\n",
        encoding="utf-8",
    )


def open_trace_in_perfetto(trace_path: Path, log: Callable[[str], None]) -> bool:
    trace_path = trace_path.resolve()
    if not trace_path.is_file():
        log(f"Perfetto trace does not exist: {trace_path}")
        return False

    handler = partial(_TraceHandler, directory=str(trace_path.parent))
    try:
        with _TraceServer(("127.0.0.1", PERFETTO_PORT), handler) as server:
            server.trace_name = trace_path.name
            server.trace_requested = False
            local_url = f"http://127.0.0.1:{PERFETTO_PORT}/{urllib.parse.quote(trace_path.name)}"
            browser_url = f"{PERFETTO_ORIGIN}/#!/?url={local_url}&referrer=gutch_benchmark"
            deadline = time.monotonic() + PERFETTO_REQUEST_TIMEOUT_SECONDS
            server.timeout = 1
            if not webbrowser.open_new_tab(browser_url):
                log(f"Could not open a browser automatically. Open: {browser_url}")
                return False
            while not server.trace_requested and time.monotonic() < deadline:
                server.handle_request()
            if not server.trace_requested:
                log("Perfetto did not request the trace within 30 seconds.")
            return server.trace_requested
    except OSError as error:
        log(f"Could not start the local Perfetto trace server: {error}")
        return False
