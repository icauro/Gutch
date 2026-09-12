FROM rust:1.97.1-bookworm
COPY --from=python:3.12-bookworm /usr/local/ /usr/local/
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
RUN python3 -m venv /opt/gutch-python
ENV PATH="/opt/gutch-python/bin:${PATH}"
COPY pyproject.toml /tmp/pyproject.toml
RUN python -c "import tomllib,subprocess; subprocess.check_call(['pip','install',*tomllib.load(open('/tmp/pyproject.toml','rb'))['project']['dependencies']])"
WORKDIR /gutch
COPY . .
# Frozen repository files have Git mode 100644, including in Windows build contexts.
RUN find tests/fixtures -type f -exec chmod 644 {} +
RUN cargo build --locked --release --manifest-path gutch-cli/Cargo.toml
RUN groupadd --gid 1000 gutch && useradd --uid 1000 --gid 1000 --create-home gutch \
    && chown -R gutch:gutch /gutch /usr/local/cargo
ENV HOME=/home/gutch
USER gutch
