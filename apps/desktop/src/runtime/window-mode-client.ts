import {
  isSafeRequestId,
  isWindowModeSnapshot,
  type WindowMode,
  type WindowModeSnapshot,
} from "./contracts";

export {
  isWindowModeSnapshot,
  type DesktopStrategy,
  type WindowMode,
  type WindowModeSnapshot,
  type WindowModeSuppression,
} from "./contracts";

export interface WindowModeClient {
  get(): Promise<WindowModeSnapshot>;
  set(mode: WindowMode): Promise<WindowModeSnapshot>;
  subscribe(
    onSnapshot: (snapshot: WindowModeSnapshot) => void,
    onInvalid?: (error: TypeError) => void,
  ): Promise<() => void>;
}

export interface WindowModeClientPorts {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen?(
    event: string,
    handler: (payload: unknown) => void,
  ): Promise<() => void>;
  createRequestId?(): string;
}

let requestSequence = 0;

function defaultRequestId(): string {
  requestSequence = (requestSequence + 1) % Number.MAX_SAFE_INTEGER;
  return `settings-mode-${Date.now().toString(36)}-${requestSequence.toString(36)}`;
}

function acceptSnapshot(value: unknown): WindowModeSnapshot {
  if (!isWindowModeSnapshot(value)) {
    throw new TypeError("桌面宠物返回了无效的窗口模式状态");
  }
  return value;
}

export function createWindowModeClient(ports: WindowModeClientPorts): WindowModeClient {
  const createRequestId = ports.createRequestId ?? defaultRequestId;
  return {
    get: async () => acceptSnapshot(await ports.invoke<unknown>("window_mode_get")),
    set: async (mode) => {
      const requestId = createRequestId();
      if (!isSafeRequestId(requestId)) {
        throw new TypeError("window mode requestId is unsafe");
      }
      return acceptSnapshot(await ports.invoke<unknown>("window_mode_set", { requestId, mode }));
    },
    subscribe: async (onSnapshot, onInvalid) => {
      if (!ports.listen) throw new TypeError("window mode event listener is unavailable");
      return ports.listen("window-mode:changed", (payload) => {
        if (isWindowModeSnapshot(payload)) onSnapshot(payload);
        else onInvalid?.(new TypeError("桌面宠物发送了无效的窗口模式状态"));
      });
    },
  };
}
