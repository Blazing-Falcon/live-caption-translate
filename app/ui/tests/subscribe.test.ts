import { beforeEach, describe, expect, it, vi } from "vitest";

const listenMock = vi.fn();

vi.mock("@tauri-apps/api/event", () => ({ listen: (...args: unknown[]) => listenMock(...args) }));

const { EVENTS, subscribe } = await import("../src/lib/events");

type Handler = (event: { payload: unknown }) => void;

function deferredListen() {
  let resolve: (unlisten: () => void) => void = () => undefined;
  const handlerRef: { current: Handler | null } = { current: null };
  listenMock.mockImplementationOnce((_name: string, handler: Handler) => {
    handlerRef.current = handler;
    return new Promise<() => void>((done) => {
      resolve = done;
    });
  });
  return { resolve: (fn: () => void) => resolve(fn), handler: () => handlerRef.current };
}

beforeEach(() => listenMock.mockReset());

describe("subscribe", () => {
  it("unlistens immediately when disposed before registration resolves", async () => {
    const pending = deferredListen();
    const handler = vi.fn();
    const dispose = subscribe(EVENTS.pipeline, handler);
    dispose();
    const unlisten = vi.fn();
    pending.resolve(unlisten);
    await Promise.resolve();
    await Promise.resolve();
    expect(unlisten).toHaveBeenCalledTimes(1);
    pending.handler()?.({ payload: 1 });
    expect(handler).not.toHaveBeenCalled();
  });

  it("delivers payloads while active and unlistens exactly once on dispose", async () => {
    const pending = deferredListen();
    const handler = vi.fn();
    const dispose = subscribe<number>(EVENTS.pipeline, handler);
    const unlisten = vi.fn();
    pending.resolve(unlisten);
    await Promise.resolve();
    await Promise.resolve();
    pending.handler()?.({ payload: 7 });
    expect(handler).toHaveBeenCalledWith(7);
    dispose();
    dispose();
    expect(unlisten).toHaveBeenCalledTimes(1);
    pending.handler()?.({ payload: 8 });
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("swallows registration failures", async () => {
    listenMock.mockImplementationOnce(() => Promise.reject(new Error("no ipc")));
    const dispose = subscribe(EVENTS.configChanged, vi.fn());
    await Promise.resolve();
    await Promise.resolve();
    expect(() => dispose()).not.toThrow();
  });

  it("uses the documented event names", () => {
    expect(EVENTS).toMatchObject({
      pipeline: "pipeline://event",
      overlayMode: "overlay://mode",
      overlayVisible: "overlay://visible",
      configChanged: "config://changed",
      modelsProgress: "models://progress",
      hotkeysError: "hotkeys://error",
    });
  });
});
