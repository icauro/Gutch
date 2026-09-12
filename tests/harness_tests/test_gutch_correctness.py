"""Guard the correctness runner's isolation and independent output oracle."""
import ast
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from tests import run
from tests.helpers.cli import execute
from tests.datasets.generate import basic
from tests.helpers.filesystem import assert_tree, remove_owned


class GutchCorrectnessTests(unittest.TestCase):
    def test_correctness_has_no_benchmark_or_legacy_dataset_dependencies(self):
        root = run.REPOSITORY / "tests"
        paths = [root / "run.py", *(root / "support").rglob("*.py"),
                 *(root / "scenarios").rglob("*.py")]
        for path in paths:
            source = path.read_text(encoding="utf-8")
            for node in ast.walk(ast.parse(source)):
                if isinstance(node, ast.Constant) and isinstance(node.value, str):
                    self.assertNotIn("test_dataset", node.value, str(path))
                if isinstance(node, ast.ImportFrom):
                    self.assertNotIn("tests.benchmarks", node.module or "", str(path))
                if isinstance(node, ast.Import):
                    for alias in node.names:
                        self.assertNotIn("tests.benchmarks", alias.name, str(path))

    def test_fast_selection_needs_no_docker_and_case_selection_is_exact(self):
        fast = run.selected_scenarios("fast", None)
        self.assertEqual([s.name for s in fast], ["capture/repeated_capture_preserves_identity"])
        local = run.selected_scenarios("local", None)
        self.assertFalse(any(s.suite == "s3" for s in local))
        selected = run.selected_scenarios("fast", ["repository/deletion_preserves_shared_artifacts"])
        self.assertEqual([s.name for s in selected], ["repository/deletion_preserves_shared_artifacts"])

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
        from tests.helpers.process import stop_process
        script = """
import sys
from pathlib import Path
from tests.helpers.cli import execute
execute([sys.executable, '-c', 'import sys; assert sys.stdin.read() == ""'],
        Path.cwd(), {}, Path(sys.argv[1]), timeout=2)
"""
        with tempfile.TemporaryDirectory() as directory:
            # Keep the outer pipe open throughout: inherited stdin would block the child.
            process = subprocess.Popen([sys.executable, "-c", script, str(Path(directory) / "stdin.log")],
                                       cwd=run.REPOSITORY, stdin=subprocess.PIPE)
            try:
                self.assertEqual(process.wait(timeout=5), 0)
            finally:
                stop_process(process)
                process.stdin.close()

    def test_scenario_process_cleans_success_and_retains_failures(self):
        from tests.helpers.cli import Case
        from tests.helpers.scenario_process import run_scenario
        module = "tests.harness_tests.gutch_scenario_fixture"
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
        from tests.helpers.cli import Case
        from tests.helpers.scenario_process import run_scenario
        module = "tests.harness_tests.gutch_scenario_fixture"
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
        root = run.REPOSITORY / "tests/scenarios"
        registered = {scenario.module for scenario in run.SCENARIOS}
        self.assertEqual(registered, {p.relative_to(root).with_suffix("").as_posix().replace("/", ".") for p in root.rglob("*.py") if p.name != "__init__.py"})
        self.assertEqual(len(registered), len(run.SCENARIOS))
        for module in registered:
            tree = ast.parse((root / (module.replace(".", "/") + ".py")).read_text(encoding="utf-8"))
            description = ast.get_docstring(tree)
            self.assertIn("Operations:", description)
            self.assertIn("Expected:", description)
            self.assertEqual([n.name for n in tree.body if isinstance(n, ast.FunctionDef)], ["run"])


if __name__ == "__main__":
    unittest.main()
