"""Run-owned Compose services with loopback ports and explicit cleanup."""
import ipaddress
import json
import os
from pathlib import Path
import subprocess
import time
from uuid import uuid4


class Services:
    def __init__(self):
        self.project = "gutch-test-" + uuid4().hex[:12]
        self.compose = Path(__file__).with_name("compose.yml")
        self.network_created = False
        self.image_created = False

    def reserve_network(self):
        identifiers = subprocess.check_output(["docker", "network", "ls", "-q"], text=True).split()
        networks = json.loads(subprocess.check_output(["docker", "network", "inspect", *identifiers], text=True)) if identifiers else []
        used = [ipaddress.ip_network(item["Subnet"]) for network in networks
                for item in (network.get("IPAM", {}).get("Config") or []) if item.get("Subnet")]
        for candidate in ipaddress.ip_network("10.241.0.0/16").subnets(new_prefix=24):
            if any(candidate.overlaps(other) for other in used if other.version == 4):
                continue
            result = subprocess.run(["docker", "network", "create", "--subnet", str(candidate),
                                     self.project], capture_output=True, text=True)
            if result.returncode == 0:
                self.network_created = True
                return
            if "overlap" not in result.stderr.lower():
                raise RuntimeError(result.stderr)
        raise RuntimeError("No free Gutch test /24 network remains in 10.241.0.0/16")

    def command(self, *arguments, capture=False):
        result = subprocess.run(["docker", "compose", "-f", str(self.compose), "-p", self.project, *arguments],
                                env={**os.environ, "GUTCH_TEST_NETWORK": self.project,
                                     "GUTCH_TEST_IMAGE": self.project + "-builder"}, check=True,
                                text=True, stdout=subprocess.PIPE if capture else None)
        return result.stdout.strip() if capture else None

    def start(self, *, builder=False):
        self.reserve_network()
        if builder:
            from .build_image import build_image
            build_image(self.project + "-builder")
            self.image_created = True
        self.command("up", "-d", *(["storage", "builder"] if builder else ["storage"]))
        address = self.command("port", "storage", "8333", capture=True)
        self.endpoint = "http://" + address
        import boto3
        from botocore.config import Config
        client = boto3.client("s3", endpoint_url=self.endpoint, aws_access_key_id="gutch-test-access",
                              aws_secret_access_key="gutch-test-secret", region_name="us-east-1",
                              config=Config(connect_timeout=2, read_timeout=2, retries={"max_attempts": 0}))
        deadline = time.monotonic() + 60
        while True:
            try:
                client.list_buckets()
                return self
            except Exception:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.5)

    def close(self):
        self.command("down", "--volumes", "--remove-orphans")
        if self.network_created:
            subprocess.run(["docker", "network", "rm", self.project], check=True)
        if self.image_created:
            subprocess.run(["docker", "image", "rm", self.project + "-builder"], check=True)
