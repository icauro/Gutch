"""Memory measurements retain evidence and distinguish unavailable counters."""
import json
import os
from types import SimpleNamespace
from unittest.mock import patch

from comparison_benchmark_suite.support.memory import ProcessMemorySampler


def test_sampler_streams_samples_and_does_not_invent_os_counters(tmp_path):
    timeline = tmp_path / "memory.jsonl"
    sampler = ProcessMemorySampler(os.getpid(), timeline)
    sampler.stopped.set()
    with patch("comparison_benchmark_suite.support.memory.psutil.Process") as process:
        process.return_value.memory_info.return_value = SimpleNamespace(rss=8192)
        sampler.start()
        result = sampler.finish()
    assert result["sample_count"] == 1
    assert result["sampled_peak_rss_bytes"] == 8192
    assert result["observed_os_peak_working_set_bytes"] is None
    assert result["sampled_peak_private_bytes"] is None
    assert json.loads(timeline.read_text())["rss_bytes"] == 8192


def test_sampler_records_failure_without_a_false_zero_measurement(tmp_path):
    import psutil
    sampler = ProcessMemorySampler(os.getpid(), tmp_path / "memory.jsonl")
    with patch("comparison_benchmark_suite.support.memory.psutil.Process", side_effect=psutil.AccessDenied()):
        sampler.start()
        result = sampler.finish()
    assert result["sampled_peak_rss_bytes"] is None
    assert "error" in result
