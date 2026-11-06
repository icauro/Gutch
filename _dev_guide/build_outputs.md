# Gutch build files

- `compiler-cache/cargo/`: Cargo library, CLI, and test compilation outputs.
- `intermediate/`: temporary source-only Linux build contexts, removed after each build.

Run build and test commands from the Gutch root. No parent repository paths
are needed. Test reports and fixtures live in `.artifacts/tests/`.
