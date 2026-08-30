import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
WORKLOADS = ROOT / "docs" / "performance" / "workloads"
EXPECTED_SCENARIOS = {
    "boot_registration",
    "idle_station",
    "localized_canister_breach",
    "corridor_pressure_breach",
    "plasma_reaction_storm",
    "turf_heat_sparse",
    "turf_heat_dense",
    "atmos_machinery_dense",
    "callback_consumer_throttled",
    "synthetic_core_matrix",
}


class PerformanceContractTests(unittest.TestCase):
    def test_ipc_benchmark_separates_transport_and_service_cases(self):
        source = (
            ROOT / "crates" / "dogmos-perf" / "benches" / "ipc_round_trip.rs"
        ).read_text(encoding="utf-8")
        self.assertIn('name: "transport_scalar_getter"', source)
        self.assertIn("fn prepare_service_world(", source)
        self.assertIn("FrontierCommit", source)
        self.assertIn("next_stage_epoch", source)

        runner = (ROOT / "tools" / "benchmark_ipc.ps1").read_text(encoding="utf-8")
        self.assertIn('ipc-round-trip-$run.status.json', runner)
        self.assertIn("$successfulStatusRecords.Count -ne $Repetitions", runner)

    def test_workload_corpus_is_complete_and_reproducible(self):
        documents = {}
        for path in WORKLOADS.glob("*.json"):
            document = json.loads(path.read_text(encoding="utf-8"))
            documents[document["id"]] = document
            self.assertEqual(document["schema_version"], 1)
            self.assertIsInstance(document["seed"], int)
            self.assertGreater(document["duration_seconds"], 0)
            self.assertTrue(document["map"])
            self.assertTrue(document["expected_markers"])
            self.assertTrue(document["correctness_assertions"])
        self.assertEqual(set(documents), EXPECTED_SCENARIOS)
        matrix = documents["synthetic_core_matrix"]
        self.assertEqual(matrix["turf_counts"], [1000, 10000, 100000, 650250])
        self.assertEqual(matrix["gas_counts"], [4, 8, 9, 20])
        self.assertEqual(matrix["active_percentages"], [0, 1, 10, 100])
        self.assertEqual(matrix["topologies"], ["corridor", "grid", "multiz"])

    def test_workload_validator_accepts_corpus_and_emits_identity_hashes(self):
        script = ROOT / "tools" / "perf" / "Invoke-DogmosWorkload.ps1"
        completed = subprocess.run(
            [
                "powershell",
                "-NoProfile",
                "-File",
                str(script),
                "-ValidateOnly",
                "-WorkloadDirectory",
                str(WORKLOADS),
            ],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        output = json.loads(completed.stdout)
        self.assertEqual(len(output), len(EXPECTED_SCENARIOS))
        self.assertTrue(all(len(item["scenario_sha256"]) == 64 for item in output))

    def test_comparison_rejects_mismatched_environment_identity(self):
        script = ROOT / "tools" / "perf" / "Compare-DogmosPerformance.ps1"
        completed = subprocess.run(
            [
                "powershell",
                "-NoProfile",
                "-File",
                str(script),
                "-SelfTestIdentityMismatch",
            ],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        result = json.loads(completed.stdout)
        self.assertFalse(result["comparable"])
        self.assertIn("map", result["mismatches"])
        self.assertIn("scenario_sha256", result["mismatches"])

    def test_process_sampler_keeps_exact_pids_and_memory_roles_separate(self):
        script = ROOT / "tools" / "perf" / "Measure-DogmosProcesses.ps1"
        dreamdaemon = subprocess.Popen(
            ["powershell", "-NoProfile", "-Command", "Start-Sleep -Seconds 10"]
        )
        server = subprocess.Popen(
            ["powershell", "-NoProfile", "-Command", "Start-Sleep -Seconds 10"]
        )
        try:
            with tempfile.TemporaryDirectory() as output:
                completed = subprocess.run(
                    [
                        "powershell",
                        "-NoProfile",
                        "-File",
                        str(script),
                        "-DreamDaemonPid",
                        str(dreamdaemon.pid),
                        "-ServerPid",
                        str(server.pid),
                        "-OutputDirectory",
                        output,
                        "-DurationSeconds",
                        "0.7",
                        "-SampleIntervalMilliseconds",
                        "100",
                    ],
                    cwd=ROOT,
                    check=False,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
                result = json.loads(completed.stdout)
                self.assertTrue(result["server_memory_is_separate"])
                self.assertNotIn("combined_memory", json.dumps(result))
                self.assertEqual(result["roles"]["dreamdaemon"]["pid"], dreamdaemon.pid)
                self.assertEqual(result["roles"]["server"]["pid"], server.pid)
                samples = Path(result["samples_path"]).read_text(encoding="utf-8-sig")
                self.assertIn("dreamdaemon", samples)
                self.assertIn("server", samples)
                self.assertIn("private_bytes", samples)
                self.assertIn("virtual_bytes", samples)
                self.assertIn("cpu_total_ticks", samples)
        finally:
            dreamdaemon.terminate()
            server.terminate()
            dreamdaemon.wait(timeout=5)
            server.wait(timeout=5)

    def test_comparison_enforces_dreamdaemon_and_tick_budgets_only(self):
        script = ROOT / "tools" / "perf" / "Compare-DogmosPerformance.ps1"
        identity = {
            "map": "MetaStation.dmm",
            "seed": 7,
            "revision": "same-controlled-revision",
            "features": ["default"],
            "byond_version": "516.1685",
            "duration_seconds": 60,
            "scenario_sha256": "a" * 64,
        }
        baseline = {
            "identity": identity,
            "summary": {
                "dreamdaemon_private_bytes": 1000,
                "server_private_bytes": 1,
                "server_tick_p95_ns": 100,
                "server_tick_p99_ns": 100,
            },
        }
        current = {
            "identity": identity,
            "summary": {
                "dreamdaemon_private_bytes": 250,
                "server_private_bytes": 1000000,
                "server_tick_p95_ns": 104,
                "server_tick_p99_ns": 109,
            },
        }
        with tempfile.TemporaryDirectory() as directory:
            baseline_path = Path(directory) / "baseline.json"
            current_path = Path(directory) / "current.json"
            baseline_path.write_text(json.dumps(baseline), encoding="utf-8")
            current_path.write_text(json.dumps(current), encoding="utf-8")
            completed = subprocess.run(
                [
                    "powershell",
                    "-NoProfile",
                    "-File",
                    str(script),
                    "-BaselinePath",
                    str(baseline_path),
                    "-CurrentPath",
                    str(current_path),
                ],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        result = json.loads(completed.stdout)
        self.assertTrue(result["acceptance_passed"])
        self.assertTrue(result["gates"]["dreamdaemon_private_bytes"]["passed"])
        self.assertEqual(result["gates"]["dreamdaemon_private_bytes"]["reduction_percent"], 75)
        self.assertEqual(result["server_private_bytes_delta_percent"], 99999900)
        self.assertNotIn("server_private_bytes", result["gates"])


if __name__ == "__main__":
    unittest.main()
