# Critical flows — smoke these every run, whatever the diff touched

1. **The CLI pipeline.** `scan` → `--save` → `scans` → `diff` → `export` →
   `import` → `verify`. This is the product; if any link breaks nothing else
   matters. (CORE-001..008)
2. **The agent's HTTP surface.** `/health` unauthenticated; every other route
   401 without a token; `POST /scans` → `GET /scans/{id}` → `/download`.
   (CORE-009, 010, 020, 021)
3. **Push and pull between two agents.** The format is one format in two
   directions, and re-pushing must stay idempotent. (CORE-033, 048)
4. **Accuracy against an external oracle.** `alloc` against `du`, hardlink
   dedupe, symlinks not followed. The README calls this a test condition.
   (CORE-039, 040, 041)
5. **The agent survives its own scan.** Added 17 September 2026 after CORE-024:
   a scan must never be able to take the process down.
