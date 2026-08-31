import assert from "node:assert/strict";
import test from "node:test";

import {
  checkWebglHealth,
  evaluateGpuHealth,
  evaluatePageWebglProbe,
  healthExitCode,
  HealthInconclusiveError,
  openCdpSocket,
  readCdpHealthState,
  requestCdp,
  selectWebglProbeTarget,
} from "./sprig-desktop-webgl-health.mjs";

function healthySystemInfo() {
  return {
    gpu: {
      auxAttributes: {
        displayType: "ANGLE_OPENGL",
        glRenderer:
          "ANGLE (Mesa/X.org, llvmpipe (LLVM 15.0.6 128 bits), OpenGL 4.5 (Core Profile) Mesa 22.3.6)",
        processCrashCount: 0,
      },
      featureStatus: { webgl: "enabled" },
    },
  };
}

function healthyPageProbe() {
  return {
    offscreenCanvas: true,
    webgl2: true,
    probeContextLost: false,
    floatColorBuffer: true,
    floatTextureLinear: true,
  };
}

class MockSocket {
  listeners = new Map();

  addEventListener(name, listener) {
    const listeners = this.listeners.get(name) ?? [];
    listeners.push(listener);
    this.listeners.set(name, listeners);
  }

  removeEventListener(name, listener) {
    this.listeners.set(
      name,
      (this.listeners.get(name) ?? []).filter(
        (candidate) => candidate !== listener,
      ),
    );
  }

  emit(name, event = {}) {
    for (const listener of [...(this.listeners.get(name) ?? [])])
      listener(event);
  }

  close() {}

  send() {}
}

test("accepts the proven GPU and page-level WebGL2 state", () => {
  assert.equal(evaluateGpuHealth(healthySystemInfo()).webgl, "enabled");
  assert.equal(evaluatePageWebglProbe(healthyPageProbe()).webgl2, true);
});

test("rejects disabled WebGL and a crashed GPU process", () => {
  const disabled = healthySystemInfo();
  disabled.gpu.featureStatus.webgl = "unavailable_software";
  assert.throws(() => evaluateGpuHealth(disabled), /WebGL is not enabled/);

  const crashed = healthySystemInfo();
  crashed.gpu.auxAttributes.processCrashCount = 2;
  assert.throws(() => evaluateGpuHealth(crashed), /crashed 2 time/);
});

test("rejects a renderer that drifted away from ANGLE OpenGL and llvmpipe", () => {
  const wrongBackend = healthySystemInfo();
  wrongBackend.gpu.auxAttributes.displayType = "ANGLE_VULKAN";
  assert.throws(
    () => evaluateGpuHealth(wrongBackend),
    /unexpected Chromium GL backend/,
  );

  const wrongRenderer = healthySystemInfo();
  wrongRenderer.gpu.auxAttributes.glRenderer =
    "ANGLE (Google, Vulkan SwiftShader)";
  assert.throws(
    () => evaluateGpuHealth(wrongRenderer),
    /unexpected Chromium WebGL renderer/,
  );
});

test("rejects absent WebGL2 and each required float extension", () => {
  const absent = healthyPageProbe();
  absent.webgl2 = false;
  assert.throws(
    () => evaluatePageWebglProbe(absent),
    /create a WebGL2 context/,
  );

  const noColorBuffer = healthyPageProbe();
  noColorBuffer.floatColorBuffer = false;
  assert.throws(
    () => evaluatePageWebglProbe(noColorBuffer),
    /EXT_color_buffer_float/,
  );

  const noFloatLinear = healthyPageProbe();
  noFloatLinear.floatTextureLinear = false;
  assert.throws(
    () => evaluatePageWebglProbe(noFloatLinear),
    /OES_texture_float_linear/,
  );
});

test("rejects a missing isolated canvas or lost probe context", () => {
  const missingOffscreen = healthyPageProbe();
  missingOffscreen.offscreenCanvas = false;
  assert.throws(
    () => evaluatePageWebglProbe(missingOffscreen),
    /lacks OffscreenCanvas/,
  );

  const lostProbe = healthyPageProbe();
  lostProbe.probeContextLost = true;
  assert.throws(
    () => evaluatePageWebglProbe(lostProbe),
    /probe context is lost/,
  );
});

test("rejects malformed GPU and page probe state", () => {
  assert.throws(() => evaluateGpuHealth({ gpu: null }), /missing or malformed/);
  const invalidCount = healthySystemInfo();
  invalidCount.gpu.auxAttributes.processCrashCount = "0";
  assert.throws(
    () => evaluateGpuHealth(invalidCount),
    /invalid GPU-process crash count/,
  );
});

test("probes a lightweight page instead of Onshape's busy CAD thread", () => {
  const onshape = {
    url: "https://cad.onshape.com/documents/abc",
    webSocketDebuggerUrl: "ws://onshape",
  };
  const startPage = {
    url: "file:///home/agent/.buzz-start.html",
    webSocketDebuggerUrl: "ws://start",
  };
  assert.equal(selectWebglProbeTarget([onshape, startPage]), startPage);
  assert.equal(selectWebglProbeTarget([onshape]), onshape);
});

test("websocket connection rejects timeout, error, and early close", async (t) => {
  await t.test("timeout", async () => {
    await assert.rejects(
      openCdpSocket("ws://timeout", {
        WebSocketImpl: class extends MockSocket {},
        deadline: Date.now() + 20,
      }),
      /connection timed out/,
    );
  });

  for (const [event, expected] of [
    ["error", /connection failed/],
    ["close", /closed before opening/],
  ]) {
    await t.test(event, async () => {
      class Socket extends MockSocket {
        constructor() {
          super();
          queueMicrotask(() => this.emit(event));
        }
      }
      await assert.rejects(
        openCdpSocket(`ws://${event}`, {
          WebSocketImpl: Socket,
          deadline: Date.now() + 100,
        }),
        expected,
      );
    });
  }
});

test("CDP request rejects timeout, socket error, close, malformed JSON, and protocol error", async (t) => {
  const request = (socket, deadline = Date.now() + 100) =>
    requestCdp(socket, "SystemInfo.getInfo", {}, { deadline, requestId: 7 });

  await t.test("timeout", async () => {
    await assert.rejects(
      request(new MockSocket(), Date.now() + 20),
      /timed out/,
    );
  });

  for (const [event, expected] of [
    ["error", /websocket failed/],
    ["close", /closed before responding/],
  ]) {
    await t.test(event, async () => {
      class Socket extends MockSocket {
        send() {
          queueMicrotask(() => this.emit(event));
        }
      }
      await assert.rejects(request(new Socket()), expected);
    });
  }

  await t.test("malformed JSON", async () => {
    class Socket extends MockSocket {
      send() {
        queueMicrotask(() => this.emit("message", { data: "{" }));
      }
    }
    await assert.rejects(request(new Socket()), /returned malformed JSON/);
  });

  await t.test("CDP protocol error", async () => {
    class Socket extends MockSocket {
      send() {
        queueMicrotask(() =>
          this.emit("message", {
            data: JSON.stringify({
              id: 7,
              error: { code: -1, message: "failed" },
            }),
          }),
        );
      }
    }
    await assert.rejects(request(new Socket()), /SystemInfo.getInfo failed/);
  });
});

test("CDP HTTP endpoint errors and malformed JSON are explicit", async () => {
  await assert.rejects(
    readCdpHealthState("http://127.0.0.1:9222", {
      fetchImpl: async () => {
        throw new Error("connection refused");
      },
      WebSocketImpl: class {},
    }),
    /is unreachable: connection refused/,
  );
  await assert.rejects(
    readCdpHealthState("http://127.0.0.1:9222", {
      fetchImpl: async () => ({ ok: false, status: 503 }),
      WebSocketImpl: class {},
    }),
    /returned HTTP 503/,
  );
  await assert.rejects(
    readCdpHealthState("http://127.0.0.1:9222", {
      fetchImpl: async () => ({
        ok: true,
        json: async () => {
          throw new Error("bad JSON");
        },
      }),
      WebSocketImpl: class {},
    }),
    /returned malformed JSON/,
  );
});

test("checkWebglHealth probes SystemInfo and an actual page target", async () => {
  class ScriptedWebSocket extends MockSocket {
    constructor(url) {
      super();
      this.url = url;
      queueMicrotask(() => this.emit("open"));
    }

    send(message) {
      const request = JSON.parse(message);
      const result =
        request.method === "SystemInfo.getInfo"
          ? healthySystemInfo()
          : { result: { value: healthyPageProbe() } };
      queueMicrotask(() =>
        this.emit("message", {
          data: JSON.stringify({ id: request.id, result }),
        }),
      );
    }
  }

  const fetchImpl = async (url) => ({
    ok: true,
    json: async () =>
      url.endsWith("/json/version")
        ? { webSocketDebuggerUrl: "ws://browser" }
        : [{ type: "page", webSocketDebuggerUrl: "ws://page" }],
  });
  const health = await checkWebglHealth("http://127.0.0.1:9222", {
    fetchImpl,
    WebSocketImpl: ScriptedWebSocket,
  });
  assert.equal(health.webgl, "enabled");
  assert.equal(health.pageProbe.floatColorBuffer, true);
});

test("a busy page timeout is inconclusive after browser GPU health passes", async () => {
  class BusyPageWebSocket extends MockSocket {
    constructor(url) {
      super();
      this.url = url;
      queueMicrotask(() => this.emit("open"));
    }

    send(message) {
      const request = JSON.parse(message);
      if (request.method === "SystemInfo.getInfo") {
        queueMicrotask(() =>
          this.emit("message", {
            data: JSON.stringify({
              id: request.id,
              result: healthySystemInfo(),
            }),
          }),
        );
      }
    }
  }

  const fetchImpl = async (url) => ({
    ok: true,
    json: async () =>
      url.endsWith("/json/version")
        ? { webSocketDebuggerUrl: "ws://browser" }
        : [{ type: "page", webSocketDebuggerUrl: "ws://busy-page" }],
  });
  await assert.rejects(
    checkWebglHealth("http://127.0.0.1:9222", {
      fetchImpl,
      WebSocketImpl: BusyPageWebSocket,
      timeoutMs: 30,
    }),
    (error) => {
      assert.ok(error instanceof HealthInconclusiveError);
      assert.match(error.message, /page WebGL probe could not answer/);
      assert.equal(healthExitCode(error), 2);
      return true;
    },
  );
});

test("a browser-wide GPU timeout remains unhealthy", async () => {
  class UnresponsiveBrowserWebSocket extends MockSocket {
    constructor() {
      super();
      queueMicrotask(() => this.emit("open"));
    }
  }

  const fetchImpl = async () => ({
    ok: true,
    json: async () => ({ webSocketDebuggerUrl: "ws://browser" }),
  });
  await assert.rejects(
    checkWebglHealth("http://127.0.0.1:9222", {
      fetchImpl,
      WebSocketImpl: UnresponsiveBrowserWebSocket,
      timeoutMs: 30,
    }),
    (error) => {
      assert.equal(error instanceof HealthInconclusiveError, false);
      assert.match(error.message, /SystemInfo\.getInfo timed out/);
      assert.equal(healthExitCode(error), 1);
      return true;
    },
  );
});

test("confirmed GPU failure remains unhealthy and skips the page probe", async () => {
  let targetEndpointRequested = false;
  class CrashedGpuWebSocket extends MockSocket {
    constructor() {
      super();
      queueMicrotask(() => this.emit("open"));
    }

    send(message) {
      const request = JSON.parse(message);
      const crashed = healthySystemInfo();
      crashed.gpu.auxAttributes.processCrashCount = 1;
      queueMicrotask(() =>
        this.emit("message", {
          data: JSON.stringify({ id: request.id, result: crashed }),
        }),
      );
    }
  }

  const fetchImpl = async (url) => {
    if (url.endsWith("/json/list")) targetEndpointRequested = true;
    return {
      ok: true,
      json: async () => ({ webSocketDebuggerUrl: "ws://browser" }),
    };
  };
  await assert.rejects(
    checkWebglHealth("http://127.0.0.1:9222", {
      fetchImpl,
      WebSocketImpl: CrashedGpuWebSocket,
    }),
    (error) => {
      assert.equal(error instanceof HealthInconclusiveError, false);
      assert.match(error.message, /GPU process has crashed 1 time/);
      assert.equal(healthExitCode(error), 1);
      return true;
    },
  );
  assert.equal(targetEndpointRequested, false);
});

test("malformed page CDP state remains unhealthy after browser GPU health passes", async () => {
  class MalformedPageWebSocket extends MockSocket {
    constructor(url) {
      super();
      this.url = url;
      queueMicrotask(() => this.emit("open"));
    }

    send(message) {
      const request = JSON.parse(message);
      queueMicrotask(() =>
        this.emit("message", {
          data:
            request.method === "SystemInfo.getInfo"
              ? JSON.stringify({ id: request.id, result: healthySystemInfo() })
              : "{",
        }),
      );
    }
  }

  const fetchImpl = async (url) => ({
    ok: true,
    json: async () =>
      url.endsWith("/json/version")
        ? { webSocketDebuggerUrl: "ws://browser" }
        : [{ type: "page", webSocketDebuggerUrl: "ws://page" }],
  });
  await assert.rejects(
    checkWebglHealth("http://127.0.0.1:9222", {
      fetchImpl,
      WebSocketImpl: MalformedPageWebSocket,
    }),
    (error) => {
      assert.equal(error instanceof HealthInconclusiveError, false);
      assert.match(error.message, /returned malformed JSON/);
      assert.equal(healthExitCode(error), 1);
      return true;
    },
  );
});

test("page protocol errors are inconclusive after browser GPU health passes", async () => {
  class NavigatingPageWebSocket extends MockSocket {
    constructor(url) {
      super();
      this.url = url;
      queueMicrotask(() => this.emit("open"));
    }

    send(message) {
      const request = JSON.parse(message);
      queueMicrotask(() =>
        this.emit("message", {
          data: JSON.stringify(
            request.method === "SystemInfo.getInfo"
              ? { id: request.id, result: healthySystemInfo() }
              : {
                  id: request.id,
                  error: {
                    code: -32000,
                    message: "Execution context destroyed",
                  },
                },
          ),
        }),
      );
    }
  }

  const fetchImpl = async (url) => ({
    ok: true,
    json: async () =>
      url.endsWith("/json/version")
        ? { webSocketDebuggerUrl: "ws://browser" }
        : [{ type: "page", webSocketDebuggerUrl: "ws://page" }],
  });
  await assert.rejects(
    checkWebglHealth("http://127.0.0.1:9222", {
      fetchImpl,
      WebSocketImpl: NavigatingPageWebSocket,
    }),
    (error) => {
      assert.ok(error instanceof HealthInconclusiveError);
      assert.match(error.message, /Execution context destroyed/);
      assert.equal(healthExitCode(error), 2);
      return true;
    },
  );
});

test("page probe exceptions are inconclusive after browser GPU health passes", async () => {
  class ExceptionalPageWebSocket extends MockSocket {
    constructor(url) {
      super();
      this.url = url;
      queueMicrotask(() => this.emit("open"));
    }

    send(message) {
      const request = JSON.parse(message);
      queueMicrotask(() =>
        this.emit("message", {
          data: JSON.stringify({
            id: request.id,
            result:
              request.method === "SystemInfo.getInfo"
                ? healthySystemInfo()
                : { exceptionDetails: { text: "page navigated" } },
          }),
        }),
      );
    }
  }

  const fetchImpl = async (url) => ({
    ok: true,
    json: async () =>
      url.endsWith("/json/version")
        ? { webSocketDebuggerUrl: "ws://browser" }
        : [{ type: "page", webSocketDebuggerUrl: "ws://page" }],
  });
  await assert.rejects(
    checkWebglHealth("http://127.0.0.1:9222", {
      fetchImpl,
      WebSocketImpl: ExceptionalPageWebSocket,
    }),
    (error) => {
      assert.ok(error instanceof HealthInconclusiveError);
      assert.match(error.message, /threw an exception/);
      assert.equal(healthExitCode(error), 2);
      return true;
    },
  );
});
