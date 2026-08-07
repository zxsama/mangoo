export async function stopThenCollectDiagnostics({ stop, waitForOutput, collect }) {
  await stop();
  await waitForOutput();
  return collect();
}
