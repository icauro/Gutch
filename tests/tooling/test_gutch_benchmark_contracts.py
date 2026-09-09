"""Failure and evidence contracts for benchmark scheduling."""
from dataclasses import replace
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
from test_benchmarks.common.dataset import DatasetSelection
from test_benchmarks.common.runner import execute_cases
from test_benchmarks.gutch_fixture_runner.models import FixtureSpec, LaneSpec, LaneResult
from test_benchmarks.support.artifacts import RunLayout
from test_benchmarks.support.command import CommandRunner, CommandError
from test_benchmarks.support.gutch import verification_seconds
from test_benchmarks.support.filesystem import compare_directories

def fixture_spec(tmp_path):
    return FixtureSpec('contract', 'local', '', True, False, (LaneSpec('first', 'filesystem'), LaneSpec('second', 'filesystem')), tmp_path / 'fixture.yaml', lane_wait_seconds=0, step_wait_seconds=0)

class GutchBenchmarkContractTests(unittest.TestCase):

    def test_failed_lane_keeps_completed_results_and_payload(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            tmp_path = Path(temporary_directory)
            layout = RunLayout(tmp_path / 'run')

            def execute(case):
                (layout.lane(case.label).payload / 'evidence').write_bytes(b'keep')
                if case.label == 'second':
                    raise RuntimeError('incorrect reconstruction')
                return LaneResult(case.label, case.kind)
            with self.assertRaisesRegex(RuntimeError, 'incorrect reconstruction'):
                execute_cases(fixture_spec(tmp_path), layout, CommandRunner(), execute, executable=Path(sys.executable))
            assert json.loads((layout.root / 'run.json').read_text(encoding='utf-8'))['status'] == 'failed'
            assert len(json.loads((layout.root / 'results.json').read_text(encoding='utf-8'))) == 1
            assert (layout.lane('second').payload / 'evidence').read_bytes() == b'keep'

    def test_warmup_and_rotated_repetitions_are_distinct(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            tmp_path = Path(temporary_directory)
            fixture = fixture_spec(tmp_path)
            fixture = replace(fixture, repetitions=2, lanes=(LaneSpec('warm', 'filesystem', warmup=True), *fixture.lanes))
            seen = []

            def execute(case):
                seen.append(case.label)
                return LaneResult(case.label, case.kind, {'capture': 1.0})
            results = execute_cases(fixture, RunLayout(tmp_path / 'run'), CommandRunner(), execute, executable=Path(sys.executable))
            assert seen == ['warm-warmup', 'first-sample-001', 'second-sample-001', 'second-sample-002', 'first-sample-002']
            assert len(results) == 4

    def test_dataset_selection_is_numeric_limited_and_frozen(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            tmp_path = Path(temporary_directory)
            for version in ('2.0.0', '10.0.0'):
                folder = tmp_path / version
                folder.mkdir()
                (folder / 'data').write_bytes(version.encode())
            dataset = DatasetSelection.resolve(tmp_path, 1)
            assert [source.name for source in dataset.sources] == ['10.0.0']
            assert dataset.input_bytes == 6
            (tmp_path / '10.0.0' / 'data').write_bytes(b'changed')
            with self.assertRaises(RuntimeError):
                dataset.validate(tmp_path / '10.0.0', tmp_path / '10.0.0', tmp_path / 'validation.json')

    def test_comparison_detects_missing_empty_directory(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            tmp_path = Path(temporary_directory)
            expected, actual = (tmp_path / 'expected', tmp_path / 'actual')
            (expected / 'empty').mkdir(parents=True)
            actual.mkdir()
            assert compare_directories(expected, actual)

    def test_invalid_verification_metrics_are_not_reported_as_zero(self):
        for value in [None, -1, float('nan'), float('inf'), True]:
            with self.assertRaises(ValueError):
                verification_seconds({'verify_timings': {'total_seconds': value}})

    def test_failed_command_retains_decodable_evidence(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            tmp_path = Path(temporary_directory)
            runner = CommandRunner(progress=False)
            runner.evidence_directory = tmp_path
            with self.assertRaises(CommandError):
                runner.run([sys.executable, '-c', "import sys; sys.stdout.buffer.write(b'\\xff'); sys.exit(7)"], description='failure')
            assert json.loads((tmp_path / '0001.json').read_text(encoding='utf-8'))['returncode'] == 7
            assert '�' in (tmp_path / '0001.stdout.log').read_text(encoding='utf-8')

    def test_borg_two_commands_use_explicit_repository(self):
        for operation, arguments, expected in [('init', ['--encryption', 'none', 'repo'], ['repo-create', '--repo', 'repo', '--encryption', 'none-sha256']), ('create', ['--compression', 'zstd,1', 'repo::snapshot', 'source'], ['create', '--repo', 'repo', '--compression', 'zstd,1', 'snapshot', 'source']), ('extract', ['repo::snapshot'], ['extract', '--repo', 'repo', 'snapshot']), ('check', ['--verify-data', 'repo'], ['check', '--repo', 'repo', '--verify-data'])]:
            from test_benchmarks.gutch_fixture_runner.external import borg_command
            command = ['borg', operation, *arguments]
            assert list(map(str, borg_command(command, Path('repo'), 2))) == ['borg', *expected]
            assert borg_command(command, Path('repo'), 1) == command

    def test_long_artifact_names_remain_bounded_distinct_and_reusable(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            tmp_path = Path(temporary_directory)
            from test_benchmarks.support.artifacts import create_run_dir
            common = 'blender-validation-' * 12
            first = create_run_dir(tmp_path, common, common + 'first')
            second = create_run_dir(tmp_path, common, common + 'second')
            assert first != second
            assert len(first.name) <= 56
            layout = RunLayout(first)
            lane = layout.lane(common + 'first')
            assert len(lane.root.name) <= 32
            assert layout.lane(common + 'first').root == lane.root
            assert layout.lane(common + 'second').root != lane.root

    def test_successful_cargo_check_accepts_release_unchanged_by_test_sources(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            tmp_path = Path(temporary_directory)
            import os
            from types import SimpleNamespace
            from test_benchmarks.gutch_fixture_runner import binary
            name = 'gutch-cli.exe' if os.name == 'nt' else 'gutch-cli'
            executable = tmp_path / '.builds/compiler-cache/cargo/release' / name
            executable.parent.mkdir(parents=True)
            executable.write_bytes(b'built')
            source = tmp_path / 'src/pipeline_tests.rs'
            source.parent.mkdir(parents=True)
            source.write_text('test-only change', encoding='utf-8')
            future = executable.stat().st_mtime + 10
            os.utime(source, (future, future))
            calls = []

            def cargo(command, **kwargs):
                calls.append(command)
                return SimpleNamespace(returncode=0)
            self.enterContext(patch.object(binary.subprocess, 'run', cargo))
            assert binary.resolve_gutch_executable(tmp_path) == executable
            assert len(calls) == 1

    def test_bulk_tools_count_and_validate_entire_product(self):
        for tool in ['restic', 'kopia', 'borg', 'plakar']:
            for missing_root_file in [False, True]:
                with tempfile.TemporaryDirectory() as temporary_directory:
                    tmp_path = Path(temporary_directory)
                    import shutil
                    from test_benchmarks.gutch_fixture_runner.external import ExternalTools
                    from test_benchmarks.gutch_fixture_runner.workflow import LocalFixtureRunner
                    from test_benchmarks.support.command import CommandResult
                    from test_benchmarks.support.commands import GutchCli
                    product = tmp_path / 'product'
                    for name in ('1.0.0', '.hidden'):
                        (product / name).mkdir(parents=True)
                        (product / name / 'data').write_bytes(b'data')
                    (product / 'root.txt').write_bytes(b'root')
                    executable = tmp_path / tool
                    executable.write_bytes(b'tool')
                    layout = RunLayout(tmp_path / 'run')
                    output = layout.lane(tool).payload / 'reconstructed'

                    class Commands:

                        def run(self, command, *, description, **kwargs):
                            if description.endswith((' restore', ' extract')):
                                if tool == 'restic':
                                    assert '--verify' in command
                                shutil.copytree(product, output, dirs_exist_ok=True)
                                if missing_root_file:
                                    (output / 'root.txt').unlink()
                            stdout = 'borg 1.4.5' if '--version' in command else '{"message_type":"summary","snapshot_id":"abc"}'
                            return CommandResult(tuple(map(str, command)), 0.01, 0, stdout, '')
                    commands = Commands()
                    runner = LocalFixtureRunner(GutchCli(executable), commands, layout, ExternalTools(tmp_path, commands, **{tool: executable}))
                    case = LaneSpec(tool, tool, workload='exact-bulk')
                    if missing_root_file:
                        with self.assertRaisesRegex(RuntimeError, 'root.txt'):
                            runner.run_lane(product, case)
                    else:
                        result = runner.run_lane(product, case)
                        assert result.metrics['input_bytes'] == 12
                        assert result.metrics['input_files'] == 3
                        assert result.metadata['dataset_sha256'] == DatasetSelection.resolve(product, whole_product=True).fingerprint
