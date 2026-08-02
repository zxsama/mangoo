/// <reference lib="webworker" />

// Decodes preview bytes into an ImageBitmap OFF the main thread.
//
// This is the whole point of the pipeline: measured on the real library, a
// full-resolution main-thread decode costs 337ms (16384px) / 323ms (10032px)
// per navigation step. Here it costs the main thread only a postMessage.
//
// The bitmap is TRANSFERRED (not copied) back, so ownership moves to the main
// thread and nothing is duplicated in memory.

type DecodeRequestMessage = { id: number; blob: Blob };

const worker = self as unknown as DedicatedWorkerGlobalScope;

worker.onmessage = async (event: MessageEvent<DecodeRequestMessage>) => {
  const { id, blob } = event.data;
  try {
    const bitmap = await createImageBitmap(blob);
    worker.postMessage({ id, ok: true, bitmap }, [bitmap]);
  } catch (cause) {
    worker.postMessage({
      id,
      ok: false,
      error: cause instanceof Error ? cause.message : String(cause)
    });
  }
};
