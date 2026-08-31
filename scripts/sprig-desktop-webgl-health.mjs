#!/usr/bin/env node
import { pathToFileURL } from "node:url";

const DEFAULT_CDP_URL = "http://127.0.0.1:9222";
const DEFAULT_TIMEOUT_MS = 2000;

class CdpCommunicationError extends Error {}
class CdpProtocolError extends Error {}

export class HealthInconclusiveError extends Error {}

export function healthExitCode(error) {
  return error instanceof HealthInconclusiveError ? 2 : 1;
}

const PAGE_WEBGL_PROBE = `(() => {
  const probeCanvas = typeof OffscreenCanvas === "function"
    ? new OffscreenCanvas(1, 1)
    : null;
  const gl = probeCanvas?.getContext("webgl2") ?? null;
  const result = {
    offscreenCanvas: Boolean(probeCanvas),
    webgl2: Boolean(gl),
    probeContextLost: gl ? gl.isContextLost() : null,
    floatColorBuffer: Boolean(gl && gl.getExtension("EXT_color_buffer_float")),
    floatTextureLinear: Boolean(gl && gl.getExtension("OES_texture_float_linear")),
  };
  // OffscreenCanvas keeps the synthetic health context isolated from the
  // application's DOM, so releasing it cannot dispatch a context-loss event
  // into Onshape or claim an otherwise unused application canvas.
  gl?.getExtension("WEBGL_lose_context")?.loseContext();
  return result;
})()`;

function requireRecord(value, label) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} is missing or malformed`);
  }
  return value;
}

function remainingMs(deadline, label) {
  const remaining = deadline - Date.now();
  if (remaining <= 0) throw new CdpCommunicationError(`${label} timed out`);
  return remaining;
}

async function fetchJson(url, { fetchImpl, deadline, label }) {
  let response;
  const signal = AbortSignal.timeout(remainingMs(deadline, label));
  try {
    response = await fetchImpl(url, {
      signal,
    });
  } catch (error) {
    throw new CdpCommunicationError(
      `${label} is unreachable: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  if (!response.ok)
    throw new CdpCommunicationError(
      `${label} returned HTTP ${response.status}`,
    );
  try {
    return await response.json();
  } catch {
    if (signal.aborted) {
      throw new CdpCommunicationError(`${label} timed out`);
    }
    throw new Error(`${label} returned malformed JSON`);
  }
}

export function evaluateGpuHealth(systemInfo) {
  const gpu = requireRecord(
    requireRecord(systemInfo, "CDP SystemInfo").gpu,
    "CDP GPU info",
  );
  const attributes = requireRecord(gpu.auxAttributes, "CDP GPU attributes");
  const features = requireRecord(gpu.featureStatus, "CDP GPU feature status");
  const displayType = attributes.displayType;
  const renderer = attributes.glRenderer;
  const processCrashCount = attributes.processCrashCount;
  const webgl = features.webgl;

  if (displayType !== "ANGLE_OPENGL") {
    throw new Error(`unexpected Chromium GL backend: ${String(displayType)}`);
  }
  if (
    typeof renderer !== "string" ||
    !/mesa/i.test(renderer) ||
    !/llvmpipe/i.test(renderer)
  ) {
    throw new Error(`unexpected Chromium WebGL renderer: ${String(renderer)}`);
  }
  if (webgl !== "enabled") {
    throw new Error(`Chromium WebGL is not enabled: ${String(webgl)}`);
  }
  if (!Number.isInteger(processCrashCount) || processCrashCount < 0) {
    throw new Error(
      `invalid GPU-process crash count: ${String(processCrashCount)}`,
    );
  }
  if (processCrashCount !== 0) {
    throw new Error(
      `Chromium GPU process has crashed ${processCrashCount} time(s)`,
    );
  }

  return { displayType, renderer, webgl, processCrashCount };
}

export function evaluatePageWebglProbe(probeResult) {
  const probe = requireRecord(probeResult, "page WebGL probe");
  if (probe.offscreenCanvas !== true) {
    throw new Error("page lacks OffscreenCanvas for an isolated WebGL2 probe");
  }
  if (probe.webgl2 !== true) {
    throw new Error("page could not create a WebGL2 context");
  }
  if (probe.probeContextLost !== false) {
    throw new Error("page WebGL2 probe context is lost");
  }
  if (probe.floatColorBuffer !== true) {
    throw new Error("page WebGL2 lacks EXT_color_buffer_float");
  }
  if (probe.floatTextureLinear !== true) {
    throw new Error("page WebGL2 lacks OES_texture_float_linear");
  }
  return probe;
}

export function selectWebglProbeTarget(pageTargets) {
  const isOnshape = (target) => /(^|\.)onshape\.com\//i.test(target.url ?? "");
  // A CAD document can keep its main thread busy for many seconds while it
  // tessellates and uploads geometry. Running the synthetic probe in that
  // thread turns healthy Onshape loading into a false timeout and causes the
  // supervisor to restart the browser. Probe a lightweight sibling page when
  // one exists; SystemInfo.getInfo still observes the browser-wide GPU process
  // and its crash counter, which is the failure boundary we recover.
  return (
    pageTargets.find(
      (target) =>
        !isOnshape(target) && /^(https?|file|chrome):/i.test(target.url ?? ""),
    ) ??
    pageTargets.find((target) => !isOnshape(target)) ??
    pageTargets[0]
  );
}

export function openCdpSocket(url, { WebSocketImpl, deadline }) {
  return new Promise((resolve, reject) => {
    let socket;
    try {
      socket = new WebSocketImpl(url);
    } catch (error) {
      reject(
        new CdpCommunicationError(
          `CDP websocket could not be created: ${error instanceof Error ? error.message : String(error)}`,
        ),
      );
      return;
    }

    const cleanup = () => {
      clearTimeout(timeout);
      socket.removeEventListener("open", onOpen);
      socket.removeEventListener("error", onError);
      socket.removeEventListener("close", onClose);
    };
    const onOpen = () => {
      cleanup();
      resolve(socket);
    };
    const onError = () => {
      cleanup();
      reject(new CdpCommunicationError("CDP websocket connection failed"));
    };
    const onClose = () => {
      cleanup();
      reject(new CdpCommunicationError("CDP websocket closed before opening"));
    };
    const timeout = setTimeout(
      () => {
        cleanup();
        socket.close();
        reject(new CdpCommunicationError("CDP websocket connection timed out"));
      },
      remainingMs(deadline, "CDP websocket connection"),
    );

    socket.addEventListener("open", onOpen);
    socket.addEventListener("error", onError);
    socket.addEventListener("close", onClose);
  });
}

export function requestCdp(socket, method, params, { deadline, requestId }) {
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(timeout);
      socket.removeEventListener("message", onMessage);
      socket.removeEventListener("error", onError);
      socket.removeEventListener("close", onClose);
    };
    const onMessage = (event) => {
      let message;
      try {
        message = JSON.parse(event.data);
      } catch {
        cleanup();
        reject(new Error(`CDP ${method} returned malformed JSON`));
        return;
      }
      if (message.id !== requestId) return;
      cleanup();
      if (message.error) {
        reject(
          new CdpProtocolError(
            `CDP ${method} failed: ${JSON.stringify(message.error)}`,
          ),
        );
      } else {
        resolve(message.result);
      }
    };
    const onError = () => {
      cleanup();
      reject(new CdpCommunicationError(`CDP ${method} websocket failed`));
    };
    const onClose = () => {
      cleanup();
      reject(
        new CdpCommunicationError(
          `CDP ${method} websocket closed before responding`,
        ),
      );
    };
    const timeout = setTimeout(
      () => {
        cleanup();
        reject(new CdpCommunicationError(`CDP ${method} timed out`));
      },
      remainingMs(deadline, `CDP ${method}`),
    );

    socket.addEventListener("message", onMessage);
    socket.addEventListener("error", onError);
    socket.addEventListener("close", onClose);
    try {
      socket.send(JSON.stringify({ id: requestId, method, params }));
    } catch (error) {
      cleanup();
      reject(
        new CdpCommunicationError(
          `CDP ${method} request failed: ${error instanceof Error ? error.message : String(error)}`,
        ),
      );
    }
  });
}

export async function readCdpHealthState(
  cdpUrl = DEFAULT_CDP_URL,
  {
    fetchImpl = globalThis.fetch,
    WebSocketImpl = globalThis.WebSocket,
    timeoutMs = DEFAULT_TIMEOUT_MS,
  } = {},
) {
  if (typeof fetchImpl !== "function" || typeof WebSocketImpl !== "function") {
    throw new Error(
      "Node runtime lacks the fetch or WebSocket API required for CDP health checks",
    );
  }
  const deadline = Date.now() + timeoutMs;
  const version = await fetchJson(`${cdpUrl}/json/version`, {
    fetchImpl,
    deadline,
    label: "CDP version endpoint",
  });
  if (
    typeof version?.webSocketDebuggerUrl !== "string" ||
    version.webSocketDebuggerUrl.length === 0
  ) {
    throw new Error("CDP version response lacks webSocketDebuggerUrl");
  }

  const browserSocket = await openCdpSocket(version.webSocketDebuggerUrl, {
    WebSocketImpl,
    deadline,
  });
  let systemInfo;
  try {
    systemInfo = await requestCdp(
      browserSocket,
      "SystemInfo.getInfo",
      {},
      {
        deadline,
        requestId: 1,
      },
    );
  } finally {
    browserSocket.close();
  }

  // Only a browser-wide result can establish that the renderer itself is
  // healthy. Once that boundary has passed, failure to schedule work on a
  // busy CAD page is inconclusive rather than evidence of a GPU failure.
  evaluateGpuHealth(systemInfo);

  try {
    const targets = await fetchJson(`${cdpUrl}/json/list`, {
      fetchImpl,
      deadline,
      label: "CDP target endpoint",
    });
    if (!Array.isArray(targets)) {
      throw new Error("CDP target response is missing or malformed");
    }
    const pageTargets = targets.filter(
      (target) =>
        target?.type === "page" &&
        typeof target.webSocketDebuggerUrl === "string",
    );
    const pageTarget = selectWebglProbeTarget(pageTargets);
    if (!pageTarget) {
      throw new CdpCommunicationError("CDP has no inspectable page target");
    }

    const pageSocket = await openCdpSocket(pageTarget.webSocketDebuggerUrl, {
      WebSocketImpl,
      deadline,
    });
    let evaluation;
    try {
      evaluation = await requestCdp(
        pageSocket,
        "Runtime.evaluate",
        { expression: PAGE_WEBGL_PROBE, returnByValue: true },
        { deadline, requestId: 2 },
      );
    } finally {
      pageSocket.close();
    }
    if (evaluation?.exceptionDetails) {
      throw new HealthInconclusiveError(
        "browser GPU is healthy but the page WebGL probe threw an exception",
      );
    }
    return { systemInfo, pageProbe: evaluation?.result?.value };
  } catch (error) {
    if (
      error instanceof CdpCommunicationError ||
      error instanceof CdpProtocolError
    ) {
      throw new HealthInconclusiveError(
        `browser GPU is healthy but the page WebGL probe could not answer: ${error.message}`,
      );
    }
    throw error;
  }
}

export async function checkWebglHealth(cdpUrl = DEFAULT_CDP_URL, dependencies) {
  const state = await readCdpHealthState(cdpUrl, dependencies);
  return {
    ...evaluateGpuHealth(state.systemInfo),
    pageProbe: evaluatePageWebglProbe(state.pageProbe),
  };
}

async function main() {
  try {
    const health = await checkWebglHealth(process.argv[2] ?? DEFAULT_CDP_URL);
    process.stdout.write(
      `WebGL2 healthy (${health.displayType}; ${health.renderer}; GPU crashes: ${health.processCrashCount}; required extensions: yes)\n`,
    );
  } catch (error) {
    process.stderr.write(
      `${error instanceof Error ? error.message : String(error)}\n`,
    );
    process.exitCode = healthExitCode(error);
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  await main();
}
