"""Guard the correctness runner's isolation and independent output oracle."""
import ast
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from tests import run_all
from tests.support.cli import execute
from tests.support.datasets import basic
from tests.support.filesystem import assert_tree, remove_owned


class GutchCorrectnessTests(unittest.TestCase):
    def test_correctness_has_no_benchmark_or_legacy_dataset_dependencies(self):
        root = run_all.REPOSITORY / "tests"
        paths = [root / "run_all.py", *(root / "support").rglob("*.py"),
                 *(root / "workflows").rglob("*.py")]
        for path in paths:
            source = path.read_text(encoding="utf-8")
            for node in ast.walk(ast.parse(source)):
                if isinstance(node, ast.Constant) and isinstance(node.value, str):
                    self.assertNotIn("test_dataset", node.value, str(path))
                if isinstance(node, ast.ImportFrom):
                    self.assertNotIn("test_benchmarks", node.module or "", str(path))
                if isinstance(node, ast.Import):
                    for alias in node.names:
                        self.assertNotIn("test_benchmarks", alias.name, str(path))

    def test_fast_selection_needs_no_docker_and_case_selection_is_exact(self):
        fast = run_all.selected_scenarios("fast", None)
        self.assertEqual([s.name for s in fast], ["capture_roundtrip"])
        local = run_all.selected_scenarios("local", None)
        self.assertFalse(any(s.suite == "s3" for s in local))
        selected = run_all.selected_scenarios("fast", ["snapshot_cleanup"])
        self.assertEqual([s.name for s in selected], ["snapshot_cleanup"])

    def test_independent_inventory_detects_same_size_corruption_and_extra_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            data = basic(root / "source")
            assert_tree(data.expected, data.root, root / "comparison.json")
            path = data.root / "data/shared.bin"
            path.write_bytes(bytes(path.stat().st_size))
            with self.assertRaisesRegex(AssertionError, "different"):
                assert_tree(data.expected, data.root, root / "comparison.json")
            data = basic(data.root)
            (data.root / "extra.txt").write_text("extra")
            with self.assertRaisesRegex(AssertionError, "unexpected"):
                assert_tree(data.expected, data.root, root / "comparison.json")

    def test_cleanup_rejects_owner_and_sibling(self):
        with tempfile.TemporaryDirectory() as directory:
            owner = Path(directory) / "owner"
            owner.mkdir()
            for target in (owner, owner.parent / "sibling"):
                with self.assertRaises(ValueError):
                    remove_owned(target, owner)

    def test_deadline_terminates_a_real_child_process(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(subprocess.TimeoutExpired):
                execute([sys.executable, "-c", "import time; time.sleep(60)"],
                        root, {}, root / "timeout.log", timeout=0.2)
            import json
            evidence = json.loads((root / "timeout.json").read_text())
            self.assertIsNotNone(evidence["exit_code"])

    def test_commands_receive_eof_even_when_runner_stdin_is_open(self):
        from tests.support.process import stop_process
        script = """
import sys
from pathlib import Path
from tests.support.cli import execute
execute([sys.executable, '-c', 'import sys; assert sys.stdin.read() == ""'],
        Path.cwd(), {}, Path(sys.argv[1]), timeout=2)
"""
        with tempfile.TemporaryDirectory() as directory:
            # Keep the outer pipe open throughout: inherited stdin would block the child.
            process = subprocess.Popen([sys.executable, "-c", script, str(Path(directory) / "stdin.log")],
                                       cwd=run_all.REPOSITORY, stdin=subprocess.PIPE)
            try:
                self.assertEqual(process.wait(timeout=5), 0)
            finally:
                stop_process(process)
                process.stdin.close()

    def test_scenario_process_cleans_success_and_retains_failures(self):
        from tests.support.cli import Case
        from tests.support.scenario_process import run_scenario
        module = "tests.tooling.gutch_scenario_fixture"
        with tempfile.TemporaryDirectory() as directory:
            for behavior, status in (("success", "passed"), ("fail", "failed"), ("skip", "skipped")):
                case = Case(Path(directory) / behavior, Path(sys.executable))
                result = run_scenario(module, case, {"GUTCH_FIXTURE_BEHAVIOR": behavior})
                self.assertEqual(result["status"], status, result)
                self.assertEqual(case.payload.exists(), status == "failed")
            case = Case(Path(directory) / "retained", Path(sys.executable))
            result = run_scenario(module, case, {"GUTCH_FIXTURE_BEHAVIOR": "success"}, keep_artifacts=True)
            self.assertEqual(result["status"], "passed")
            self.assertTrue(case.payload.is_dir())

    def test_ten_second_deadline_kills_worker_and_cli_child_then_next_scenario_runs(self):
        import time
        import psutil
        from tests.support.cli import Case
        from tests.support.scenario_process import run_scenario
        module = "tests.tooling.gutch_scenario_fixture"
        with tempfile.TemporaryDirectory() as directory:
            case = Case(Path(directory) / "blocked", Path(sys.executable))
            started = time.monotonic()
            result = run_scenario(module, case, {"GUTCH_FIXTURE_BEHAVIOR": "timeout"})
            elapsed = time.monotonic() - started
            self.assertEqual(result["status"], "failed")
            self.assertTrue(result["timed_out"])
            self.assertGreaterEqual(elapsed, 10)
            self.assertLess(elapsed, 16)
            self.assertTrue(case.payload.is_dir())
            child_pid = int((case.root / "child.pid").read_text())
            deadline = time.monotonic() + 2
            while psutil.pid_exists(child_pid) and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertFalse(psutil.pid_exists(child_pid))
            following = Case(Path(directory) / "following", Path(sys.executable))
            self.assertEqual(run_scenario(module, following, {"GUTCH_FIXTURE_BEHAVIOR": "success"})["status"], "passed")

    def test_each_registered_scenario_has_its_own_documented_file(self):
        root = run_all.REPOSITORY / "tests/workflows"
        registered = {scenario.module for scenario in run_all.SCENARIOS}
        self.assertEqual(registered, {p.stem for p in root.glob("*.py") if p.name != "__init__.py"})
        self.assertEqual(len(registered), len(run_all.SCENARIOS))
        for module in registered:
            tree = ast.parse((root / f"{module}.py").read_text(encoding="utf-8"))
            description = ast.get_docstring(tree)
            self.assertIn("Operations:", description)
            self.assertIn("Expected:", description)
            self.assertEqual([n.name for n in tree.body if isinstance(n, ast.FunctionDef)], ["run"])


if __name__ == "__main__":
    unittest.main()
