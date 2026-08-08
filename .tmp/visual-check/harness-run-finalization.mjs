function finishAndObserveStream(name, stream) {
  return new Promise((resolve) => {
    let finished = stream.writableFinished === true;
    let closed = stream.closed === true;
    let error = null;

    const settle = () => {
      if (!closed || (!finished && !error)) return;
      stream.off("finish", onFinish);
      stream.off("close", onClose);
      stream.off("error", onError);
      resolve({ name, finished, closed, error });
    };
    const onFinish = () => {
      finished = true;
      settle();
    };
    const onClose = () => {
      closed = true;
      settle();
    };
    const onError = (streamError) => {
      error = String(streamError);
      if (!stream.destroyed) stream.destroy();
      settle();
    };

    stream.on("finish", onFinish);
    stream.on("close", onClose);
    stream.on("error", onError);
    if (!stream.writableEnded) stream.end();
    settle();
  });
}

async function finalizeHarnessDiagnostics({
  stop,
  waitForChildClose,
  outputStreams,
  collect
}) {
  await stop();
  const child = await waitForChildClose();
  const streamStates = await Promise.all(
    outputStreams.map(({ name, stream }) => finishAndObserveStream(name, stream))
  );
  const stateByName = Object.fromEntries(streamStates.map((state) => [state.name, state]));
  const finalization = {
    childCloseObserved: child?.closeObserved === true,
    childExitCode: child?.code ?? null,
    childSignal: child?.signal ?? null,
    stdoutFinished: stateByName.stdout?.finished === true,
    stdoutClosed: stateByName.stdout?.closed === true,
    stderrFinished: stateByName.stderr?.finished === true,
    stderrClosed: stateByName.stderr?.closed === true,
    streamErrors: streamStates.flatMap((state) => state.error ? [`${state.name}: ${state.error}`] : [])
  };
  finalization.complete = finalization.childCloseObserved &&
    finalization.stdoutFinished && finalization.stdoutClosed &&
    finalization.stderrFinished && finalization.stderrClosed &&
    finalization.streamErrors.length === 0;

  const diagnostics = collect();
  if (!finalization.complete) {
    diagnostics.hardFailures = [
      ...new Set([...(diagnostics.hardFailures ?? []), "child/output finalization incomplete"])
    ];
  }
  return { ...diagnostics, finalization };
}

export function finalizeNormalOsDiagnostics(options) {
  return finalizeHarnessDiagnostics(options);
}

export function stopThenCollectDiagnostics(options) {
  return finalizeHarnessDiagnostics(options);
}
