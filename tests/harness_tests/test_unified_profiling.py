"""Unified profiling configuration and complete-duration trace composition."""

import json
from pathlib import Path
from tests.benchmarks.support.commands import GutchCli
from tests.benchmarks.support.traces import combine_labeled_traces
from tests.benchmarks.comparison.report import _read_trace_spans


def test_cli_profile_modes_have_one_switch(tmp_path):
    for mode in ("off", "summary", "detailed"):
        command = GutchCli(Path("gutch"), profile_mode=mode).command(
            "snapshot", "capture", "--profile-dir", tmp_path
        )
        assert command[-2:] == ["--profile", mode]
        assert ("--profile-dir" in command) == (mode != "off")


def test_complete_events_do_not_overlap_sequential_operations(tmp_path):
    paths = []
    for index in range(2):
        path = tmp_path / f"{index}.json"
        path.write_text(
            json.dumps(
                {
                    "traceEvents": [
                        {
                            "name": "capture",
                            "ph": "X",
                            "ts": 10,
                            "dur": 100,
                            "pid": 1,
                            "tid": 1,
                            "args": {},
                        }
                    ]
                }
            )
        )
        paths.append(("lane", path))
    output = tmp_path / "combined.json"
    combine_labeled_traces(paths, output)
    events = [
        e for e in json.loads(output.read_text())["traceEvents"] if e["ph"] == "X"
    ]
    assert events[1]["ts"] >= events[0]["ts"] + events[0]["dur"]
    assert events[0]["tid"] != events[1]["tid"]
    assert _read_trace_spans(paths[0][1]) == [("capture", 0.0001, {})]


def test_named_rows_reuse_across_commands_but_not_lanes(tmp_path):
    paths = []
    for index, tid in enumerate((4, 9)):
        path = tmp_path / f"named-{index}.json"
        path.write_text(
            json.dumps(
                {
                    "traceEvents": [
                        {
                            "ph": "M",
                            "name": "thread_name",
                            "pid": 1,
                            "tid": tid,
                            "args": {
                                "name": (
                                    "Capture worker #1",
                                    "Reconstruction worker #1",
                                )[index]
                            },
                        },
                        {
                            "ph": "X",
                            "name": "work",
                            "pid": 1,
                            "tid": tid,
                            "ts": 2,
                            "dur": 8,
                        },
                    ]
                }
            )
        )
        paths.append(("lane", path))
    output = tmp_path / "combined.json"
    combine_labeled_traces(paths + [("other", paths[0][1])], output)
    events = json.loads(output.read_text())["traceEvents"]
    spans = [event for event in events if event["ph"] == "X"]
    first, other, second = spans
    assert first["pid"] == second["pid"] != other["pid"]
    assert first["tid"] == second["tid"]
    assert second["ts"] > first["ts"] + first["dur"]
    assert all(event["dur"] == 8 for event in spans)


def test_duplicate_thread_names_do_not_merge_concurrent_workers(tmp_path):
    source = tmp_path / "source.json"
    source.write_text(
        json.dumps(
            {
                "traceEvents": [
                    event
                    for tid in (1, 2)
                    for event in (
                        {
                            "ph": "M",
                            "name": "thread_name",
                            "pid": 1,
                            "tid": tid,
                            "args": {"name": "worker"},
                        },
                        {
                            "ph": "X",
                            "name": "work",
                            "pid": 1,
                            "tid": tid,
                            "ts": 0,
                            "dur": 10,
                        },
                    )
                ]
            }
        )
    )
    output = tmp_path / "combined.json"
    combine_labeled_traces([("lane", source)], output)
    spans = [
        event
        for event in json.loads(output.read_text())["traceEvents"]
        if event["ph"] == "X"
    ]
    assert spans[0]["tid"] != spans[1]["tid"]


def test_role_rows_share_only_when_activity_does_not_overlap(tmp_path):
    for start, shared in ((10, True), (5, False)):
        source = tmp_path / f"roles-{start}.json"
        events = []
        for tid, name, timestamp in (
            (1, "Verification worker #1", 0),
            (2, "Reconstruction worker #1", start),
        ):
            events.extend(
                [
                    {
                        "ph": "M",
                        "name": "thread_name",
                        "pid": 1,
                        "tid": tid,
                        "args": {"name": name},
                    },
                    {
                        "ph": "X",
                        "name": "work",
                        "pid": 1,
                        "tid": tid,
                        "ts": timestamp,
                        "dur": 10,
                    },
                ]
            )
        source.write_text(json.dumps({"traceEvents": events}))
        output = tmp_path / "combined.json"
        combine_labeled_traces([("lane", source)], output)
        spans = [
            event
            for event in json.loads(output.read_text())["traceEvents"]
            if event["ph"] == "X"
        ]
        assert (spans[0]["tid"] == spans[1]["tid"]) == shared
