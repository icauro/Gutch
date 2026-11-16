# Document schemas

`current/` holds JSON Schemas for the documents this release stores, generated
from the Rust types by `src/tests/document_schemas.rs`. Regenerate them after a
deliberate document change with:

```
GUTCH_UPDATE_SCHEMAS=1 cargo test --lib document_schemas
```

The Gutch parsers are authoritative. They also enforce rules a schema cannot
express, such as valid packaging pairs and tag formats. Each release copies
`current/` to `v<major>/`, which is never changed afterwards. See
`_docs/current_state/16_format_evolution.md`.
