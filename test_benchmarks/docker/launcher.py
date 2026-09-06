"""Docker process/copy boundary for isolated Linux benchmark runs."""
from dataclasses import dataclass
from pathlib import Path
import os
import subprocess
from tests.common import _test_environment as environments
from tests.common._test_runtime_mode import build_mode_runtime_context
from tests.common._e2e_harness import start_backend_integration_stack
from gutch_repo.test_benchmarks.support.command import CommandRunner


@dataclass(frozen=True)
class DockerTarget:
    container: str
    workspace: str = "/tmp/hostapp-benchmark"

    @classmethod
    def start(cls, config, repo_root):
        context = build_mode_runtime_context(repo_root, lane_name='gutch-docker-benchmark',
            override_env_var='HOSTAPP_GUTCH_BENCHMARK_ENV_FILE', instance_index=0, mode='full')
        start_backend_integration_stack(context, services=['seaweedfs'],
                                       wait_for_services=['seaweedfs'], rebuild=False)
        for key, value in context.backend_values.items():
            if key.startswith('S3_MAIN_'):
                config.environment[key] = value
        config.environment['S3_MAIN_ENDPOINT'] = 'http://seaweedfs:8333'
        from scripts.build._builder import project_name
        os.environ['LINUX_BUILDER_IMAGE'] = project_name(repo_root) + '-builder'
        data = environments.read_manifest(Path(os.environ[environments.MANIFEST_ENV]))
        subprocess.run(environments.compose_command(data, 'up', '-d', '--no-deps', 'linux-benchmark'),
                       env=environments.compose_environment(data), check=True)
        identifier = subprocess.check_output(environments.compose_command(data, 'ps', '-q', 'linux-benchmark'),
                       env=environments.compose_environment(data), text=True).strip()
        return cls(identifier)


class DockerLauncher:
    def __init__(self, target, commands=None):
        self.target = target
        self.commands = commands or CommandRunner()

    def run(self, command, *, description, on_output=None):
        command = list(command)
        if command[0] in ('python', 'python3'):
            command = ['uv', 'run', '--project', '/repository', 'python', *command[1:]]
        return self.commands.run(['docker', 'exec', '--user', '1000:1000', self.target.container, *command],
                                 description=description, on_output=on_output)

    def upload(self, source, destination, *, description):
        result = self.commands.run(['docker', 'cp', str(source), f'{self.target.container}:{destination}'], description=description)
        self.commands.run(['docker', 'exec', '--user', '0', self.target.container, 'chown', '1000:1000', destination], description='Set benchmark file owner')
        return result

    def download(self, source, destination, *, description):
        return self.commands.run(['docker', 'cp', f'{self.target.container}:{source}', str(destination)], description=description)
