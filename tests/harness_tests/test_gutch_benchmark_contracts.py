"""Failure and evidence contracts for benchmark scheduling."""
from dataclasses import replace
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
from tests.benchmarks.scheduling.dataset import DatasetSelection
from tests.benchmarks.scheduling.runner import execute_cases
from tests.benchmarks.comparison.models import FixtureSpec, LaneSpec, LaneResult
from tests.benchmarks.support.artifacts import RunLayout
from tests.benchmarks.support.command import CommandRunner, CommandError
from tests.benchmarks.support.gutch import verification_seconds
from tests.benchmarks.support.filesystem import compare_directories

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
            from tests.benchmarks.comparison.external import borg_command
            command = ['borg', operation, *arguments]
            assert list(map(str, borg_command(command, Path('repo'), 2))) == ['borg', *expected]
            assert borg_command(command, Path('repo'), 1) == command

    def test_long_artifact_names_remain_bounded_distinct_and_reusable(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            tmp_path = Path(temporary_directory)
            from tests.benchmarks.support.artifacts import create_run_dir
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
            from tests.benchmarks.comparison import binary
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
                    from tests.benchmarks.comparison.external import ExternalTools
                    from tests.benchmarks.comparison.workflow import LocalFixtureRunner
                    from tests.benchmarks.support.command import CommandResult
                    from tests.benchmarks.support.commands import GutchCli
                    product = tmp_path / 'product'
                    for name in ('1.0.0', '.hidden'):
                        (product / name).mkdir(parents=True)
                        (product / name / 'data').write_bytes(b'data')
                    (product / 'root.txt').write_bytes(b'root')
                    executable = tmp_path / tool
                    executable.write_bytes(b'tool')
                    layout = RunLayout(tmp_path / 'run')
                    output = layout.lane(tool).payload / 'installation'

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


class SnapshotComparisonTests(unittest.TestCase):
    def test_filesystem_uses_selected_snapshots_and_restores_from_storage(self):
        import shutil
        from tests.benchmarks.comparison.workflow import LocalFixtureRunner
        from tests.benchmarks.support.commands import GutchCli
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            product = root / 'product'
            for name in ('1.0.0', '2.0.0', '10.0.0', '.hidden'):
                source = product / name
                source.mkdir(parents=True)
                (source / 'data').write_bytes(b'payload')
            (product / 'root.txt').write_bytes(b'excluded')
            layout = RunLayout(root / 'run')
            runner = LocalFixtureRunner(GutchCli(Path('unused')), CommandRunner(), layout)
            original_copy = shutil.copytree
            copies = []

            def copy(source, destination, **kwargs):
                copies.append((Path(source), Path(destination)))
                return original_copy(source, destination, **kwargs)

            with patch('tests.benchmarks.comparison.workflow.shutil.copytree', side_effect=copy):
                result = runner.run_lane(product, LaneSpec('copy', 'filesystem'), True, snapshot_limit=2)
            lane = layout.lane('copy')
            assert [source.name for source, _ in copies[:2]] == ['2.0.0', '10.0.0']
            assert [source.name for source, _ in copies[2:4]] == ['10.0.0', '2.0.0']
            assert all(source.parent == lane.payload / 'repo' for source, _ in copies[2:])
            assert len(copies) == 6  # Capture, initial restore, then recovery.
            assert result.metrics['repo_size'] == result.metrics['input_bytes'] == 14
            assert result.metrics['repo_files'] == 2
            assert result.metadata['dataset_sha256'] == DatasetSelection.resolve(product, 2).fingerprint
            snapshots = result.metadata['snapshot_metrics']
            assert sum(row['backup'] for row in snapshots.values()) == result.metrics['backup']
            assert sum(row['restore'] for row in snapshots.values()) == result.metrics['restore']
            for name in snapshots:
                assert (lane.payload / 'installation' / name / 'data').read_bytes() == b'payload'

    def test_filesystem_validates_initial_restore_before_recovery(self):
        import shutil
        from tests.benchmarks.comparison.workflow import LocalFixtureRunner
        from tests.benchmarks.support.commands import GutchCli
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'product' / '1.0.0'
            source.mkdir(parents=True)
            (source / 'data').write_bytes(b'payload')
            runner = LocalFixtureRunner(GutchCli(Path('unused')), CommandRunner(), RunLayout(root / 'run'))
            original_copy = shutil.copytree

            def broken_copy(source, destination, **kwargs):
                original_copy(source, destination, **kwargs)
                if Path(destination).parent.name == 'installation':
                    (Path(destination) / 'data').write_bytes(b'bad')

            with patch('tests.benchmarks.comparison.workflow.shutil.copytree', side_effect=broken_copy), patch(
                'tests.benchmarks.comparison.workflow.build_corruption_plan'
            ) as recovery:
                with self.assertRaises(RuntimeError):
                    runner.run_lane(source.parent, LaneSpec('copy', 'filesystem'), True)
                recovery.assert_not_called()

    def test_report_shows_snapshot_averages_and_weighted_throughput(self):
        import io
        from contextlib import redirect_stdout
        from tests.benchmarks.comparison.report import print_comparison_table
        result = LaneResult('filesystem-copy', 'filesystem-copy',
            {'input_bytes': 8 * 1048576, 'backup': 4.0, 'restore': 10.0, 'repo_size': 8 * 1048576,
             'corrupt': 0.0, 'full_restore_recovery': 0.0}, metadata={'snapshot_metrics': {
                 '1.0.0': {'input_bytes': 1048576, 'backup': 1.0, 'restore': 4.0},
                 '2.0.0': {'input_bytes': 7 * 1048576, 'backup': 3.0, 'restore': 6.0}}})
        output = io.StringIO()
        with redirect_stdout(output):
            print_comparison_table(fixture_spec(Path('.')), Path('product'), [result])
        report = output.getvalue()
        for value in ['Snapshot: 1.0.0', 'Snapshot: 2.0.0', '100.0 %']:
            assert value in report
        rows = {line.split('|')[1].strip(): line for line in report.splitlines() if line.startswith('|')}
        assert '2.00 s' in rows['Capture average']
        assert '5.00 s' in rows['Reconstruct average']
        assert '2.0 MiB/s' in rows['Capture speed']
        assert '8.00 MiB' in rows['Stored size (final)']
        for label in ['Snapshot capture min', 'Copy time', 'Repo files', 'Corrupt time', 'Full restore recovery']:
            assert label not in report

    def test_batch_report_does_not_invent_per_snapshot_capture_times(self):
        import io
        from contextlib import redirect_stdout
        from tests.benchmarks.comparison.report import print_comparison_table
        result = LaneResult('batch', 'gutch-batch', {'backup': 4.0}, metadata={'snapshot_metrics': {
            '1.0.0': {'input_bytes': 0, 'restore': 0.5}}})
        output = io.StringIO()
        with redirect_stdout(output):
            print_comparison_table(fixture_spec(Path('.')), Path('product'), [result])
        rows = {line.split('|')[1].strip(): line.split('|')[2].strip()
                for line in output.getvalue().splitlines() if line.startswith('|')}
        assert rows['Capture time'] == '-'
        assert rows['Capture average'] == '-'
        assert rows['Capture total'] == '4.00 s'
