- source_spec: `docs/spec/spec-fix-persistent-chromium-locks.md`
  summary: Gate PC availability and watchdog health on Chromium CDP readiness rather than container/process presence alone.
  evidence: The broker currently announces a sandbox immediately after Docker starts, and the supervisor only logs/retries process exits; this predates stale-lock recovery but can present a browserless PC as ready when Chromium fails for another reason.
