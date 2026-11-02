// A private wire peer for Host transport tests; never a discoverable plugin.
const assert = require('node:assert/strict');
const { createHash } = require('node:crypto');
const { once } = require('node:events');
const { setTimeout: delay } = require('node:timers/promises');
const mode = process.argv[2];
let buffered = Buffer.alloc(0);
const input = process.stdin[Symbol.asyncIterator]();
async function read() {
  while (buffered.length < 17 || buffered.length < 17 + buffered.readUInt32LE(0)) {
    const next = await input.next();
    if (next.done) return null;
    buffered = Buffer.concat([buffered, next.value]);
    assert.ok(buffered.length <= 512 * 1024, 'peer input must stay bounded');
  }
  const length = buffered.readUInt32LE(0);
  const frame = { kind: buffered[4], id: Number(buffered.readBigUInt64LE(5)),
    sequence: buffered.readUInt32LE(13), bytes: buffered.subarray(17, 17 + length) };
  buffered = buffered.subarray(17 + length);
  return frame;
}
async function send(kind, id, sequence, bytes) {
  const header = Buffer.alloc(17);
  header.writeUInt32LE(bytes.length); header[4] = kind;
  header.writeBigUInt64LE(BigInt(id), 5); header.writeUInt32LE(sequence, 13);
  if (!process.stdout.write(Buffer.concat([header, bytes]))) await once(process.stdout, 'drain');
}
async function control(value) { await send(1, 0, 0, Buffer.from(JSON.stringify(value))); }
async function credit(id, value) {
  const bytes = Buffer.alloc(8); bytes.writeBigUInt64LE(BigInt(value));
  await send(4, id, 0, bytes);
}
async function run() {
  const open = JSON.parse((await read()).bytes);
  assert.equal(open.method, 'stream.open');
  const size = open.payload.size, id = open.payload.streamId;
  const request = JSON.parse((await read()).bytes);
  assert.equal(request.method, 'decoder.decode');
  if (mode === 'no-credit') {
    await control({ ...request, messageType: 'response', payload: { streamId: 2 } });
    assert.equal(await read(), null, 'no source bytes before credit');
    return;
  }
  let received = 0, sequence = 1, granted = Math.min(size, 64 * 1024), largest = 0;
  const hash = createHash('sha256');
  await credit(id, granted);
  while (received < size) {
    const frame = await read();
    assert.equal(frame.kind, 2); assert.equal(frame.id, id); assert.equal(frame.sequence, sequence++);
    received += frame.bytes.length; largest = Math.max(largest, frame.bytes.length);
    assert.ok(received <= granted, 'Host exceeded actual input credit');
    assert.ok(frame.bytes.length <= 256 * 1024, 'Host exceeded chunk budget');
    hash.update(frame.bytes);
    if (granted < size) {
      await delay(2); // The Host must wait instead of reading the entire source ahead.
      granted = Math.min(size, received + 64 * 1024); await credit(id, granted);
    }
  }
  const end = await read();
  assert.equal(end.kind, 3); assert.equal(end.sequence, sequence);
  assert.equal(Number(end.bytes.readBigUInt64LE()), size);
  assert.equal(hash.digest('hex'), open.payload.sha256);
  assert.equal(end.bytes.subarray(8).toString('hex'), open.payload.sha256);
  const raster = Buffer.from([17, 83, 211, 255]);
  const digest = createHash('sha256').update(raster).digest();
  const output = { ...request, messageType: 'request', method: 'stream.open',
    payload: { streamId: 2, size: 4, sha256: digest.toString('hex') } };
  await control(output);
  await control({ ...request, messageType: 'response', payload: { streamId: 2, received, largest } });
  // Wait for an explicit test barrier after the Host consumes the response.
  let frame;
  while ((frame = await read()) !== null && frame.kind !== 6) assert.equal(frame.kind, 4);
  if (!frame) return;
  if (mode === 'extra-output') {
    await control({ ...output, payload: { ...output.payload, streamId: 4 } });
  } else {
    await send(2, 2, 1, raster);
    if (mode !== 'late-end') {
      const endBytes = Buffer.alloc(40); endBytes.writeBigUInt64LE(4n); digest.copy(endBytes, 8);
      if (mode === 'bad-hash') endBytes[8] ^= 1;
      await send(3, 2, 2, endBytes);
    }
  }
  // EOF is the Host's close/join signal. Never spawn detached work.
  while (await read()) {}
}
run().catch(error => { console.error(error.message); process.exitCode = 1; });
