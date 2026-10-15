"""Optional JSONL progress output for callers of the standalone runner."""

from datetime import datetime, timezone
import json
import time


class RunEventWriter:
    def __init__(self, path, run_id, suite_id="gutch"):
        self.path, self.run_id, self.suite_id = path, run_id, suite_id

    def emit(self, event_type, *, phase_id="", phase_name="", status="", details=None):
        if self.path is None:
            return
        event = {
            "schemaVersion": 1,
            "eventType": event_type,
            "runId": self.run_id,
            "suiteId": self.suite_id,
            "source": "runner",
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "monotonicSeconds": round(time.monotonic(), 6),
        }
        event.update(
            {
                key: value
                for key, value in {
                    "phaseId": phase_id,
                    "phaseName": phase_name,
                    "status": status,
                    "details": details,
                }.items()
                if value
            }
        )
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.path.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(event) + "\n")

    def phase_plan(self, phases):
        self.emit(
            "phase_plan",
            details={"phases": [{"id": key, "name": name} for key, name in phases]},
        )
