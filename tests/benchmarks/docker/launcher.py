"""Docker process/copy boundary for isolated Linux benchmark runs."""
from dataclasses import dataclass
from comparison_benchmark_suite.support.command import CommandRunner


@dataclass
class DockerTarget:
    container: str
    workspace: str = "/tmp/gutch-benchmark"
    services: object = None

    @classmethod
    def start(cls, config, repo_root):
        from tools.services import Services
        services = Services()
        try:
            services.start(builder=True)
            config.environment.update({"GUTCH_S3_ENDPOINT": "http://storage:8333",
                "GUTCH_S3_ACCESS_KEY": "gutch-test-access", "GUTCH_S3_SECRET_KEY": "gutch-test-secret",
                "GUTCH_S3_BUCKET_NAME": "gutch-tests", "GUTCH_S3_REGION": "us-east-1"})
            identifier = services.command("ps", "-q", "builder", capture=True)
            return cls(identifier, services=services)
        except BaseException:
            services.close()
            raise

    def close(self):
        self.services.close()


class DockerLauncher:
    def __init__(self, target, commands=None):
        self.target = target
        self.commands = commands or CommandRunner()

    def run(self, command, *, description, on_output=None):
        command = list(command)
        if command[0] in ('python', 'python3'):
            command = ['python', *command[1:]]
        return self.commands.run(['docker', 'exec', self.target.container, *command],
                                 description=description, on_output=on_output)

    def upload(self, source, destination, *, description):
        result = self.commands.run(['docker', 'cp', str(source), f'{self.target.container}:{destination}'], description=description)
        self.commands.run(['docker', 'exec', '--user', '0', self.target.container, 'chown', '1000:1000', destination], description='Set benchmark file owner')
        return result

    def download(self, source, destination, *, description):
        return self.commands.run(['docker', 'cp', f'{self.target.container}:{source}', str(destination)], description=description)
