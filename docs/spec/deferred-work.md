- source_spec: `docs/spec/spec-fix-persistent-chromium-locks.md`
  summary: Gate PC availability and watchdog health on Chromium CDP readiness rather than container/process presence alone.
  evidence: The broker currently announces a sandbox immediately after Docker starts, and the supervisor only logs/retries process exits; this predates stale-lock recovery but can present a browserless PC as ready when Chromium fails for another reason.
- source_spec: `docs/spec/spec-fix-onshape-webgl.md`
  summary: Target graceful Chromium shutdown and profile ownership checks to the supervised browser profile instead of every Chrome-family process in the container.
  evidence: The pre-existing supervisor shutdown and profile-lock helpers use process names globally, so an unrelated manually launched profile can be signaled or can block recovery even though this WebGL change did not introduce that ownership model.
