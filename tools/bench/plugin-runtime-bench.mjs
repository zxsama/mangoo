import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const CONTROL = 1;
const BINARY = 2;
const END = 3;
const CREDIT = 4;
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const built = spawnSync("node", ["tools/build/build-bundled-plugins.mjs"], { cwd: root, stdio: "inherit", shell: process.platform === "win32" });
if (built.status !== 0) process.exit(built.status ?? 1);
const targetRoot = process.env.CARGO_TARGET_DIR
  ? path.resolve(root, process.env.CARGO_TARGET_DIR)
  : path.join(root, "target");
const binary = path.join(targetRoot, "debug", "bundled-plugins", "com.megle.fake-sync", "bin", process.platform === "win32" ? "megle-fake-provider.exe" : "megle-fake-provider");
const child = spawn(binary, [], { cwd: path.dirname(binary), stdio: ["pipe", "pipe", "inherit"], windowsHide: true });
let buffered = Buffer.alloc(0);
let nextStreamId = 1;
const pending = new Map();
const credits = new Map();
const creditWaiters = new Map();
const incoming = new Map();
let providerCalls = 0;

function frame(kind, streamId, sequence, payload) {
  const header = Buffer.alloc(17);
  header.writeUInt32LE(payload.length); header[4] = kind; header.writeBigUInt64LE(BigInt(streamId), 5); header.writeUInt32LE(sequence, 13);
  return Buffer.concat([header, payload]);
}
function u64(value) { const buffer = Buffer.alloc(8); buffer.writeBigUInt64LE(BigInt(value)); return buffer; }
function sendControl(message) { child.stdin.write(frame(CONTROL, 0, 0, Buffer.from(JSON.stringify(message)))); }
child.stdout.on("data", chunk => {
  buffered = Buffer.concat([buffered, chunk]);
  while (buffered.length >= 17) {
    const size = buffered.readUInt32LE(0);
    if (buffered.length < 17 + size) break;
    const kind = buffered[4]; const streamId = Number(buffered.readBigUInt64LE(5)); const sequence = buffered.readUInt32LE(13); const payload = buffered.subarray(17, 17 + size);
    buffered = buffered.subarray(17 + size);
    if (kind === CONTROL) {
      const message = JSON.parse(payload);
      if (message.messageType === "request" && message.method === "stream.open") {
        const { streamId:id, size:objectSize, sha256 } = message.payload;
        const granted = Math.min(objectSize, 8 * 1024 * 1024);
        incoming.set(id, { size:objectSize, sha256, sequence:1, chunks:[], received:0, granted, complete:false });
        child.stdin.write(frame(CREDIT, id, 0, u64(granted)));
      } else { pending.get(message.requestId)?.(message); pending.delete(message.requestId); }
    }
    else if (kind === CREDIT) { credits.set(streamId, Number(payload.readBigUInt64LE())); creditWaiters.get(streamId)?.(); creditWaiters.delete(streamId); }
    else if (kind === BINARY) {
      const object = incoming.get(streamId); if (!object || object.sequence !== sequence) throw new Error("invalid provider stream sequence");
      object.sequence += 1; object.received += payload.length; object.chunks.push(payload);
      const next = Math.min(object.size, object.received + 8 * 1024 * 1024);
      if (next > object.granted) { object.granted = next; child.stdin.write(frame(CREDIT, streamId, 0, u64(next))); }
    } else if (kind === END) { const object = incoming.get(streamId); if (!object || object.sequence !== sequence) throw new Error("invalid provider stream end"); object.complete = true; }
  }
});
function call(method, payload, bindingEpoch = null, transportGeneration = null) {
  providerCalls += 1;
  return new Promise((resolve, reject) => {
    const requestId = crypto.randomUUID();
    pending.set(requestId, message => message.messageType === "error" ? reject(new Error(JSON.stringify(message.payload))) : resolve(message.payload));
    sendControl({ wireVersion:1,messageType:"request",requestId,method,deadlineMs:5000,bindingEpoch,transportGeneration,payload });
    setTimeout(() => { if (pending.delete(requestId)) reject(new Error(`${method} timed out`)); }, 5000).unref();
  });
}
async function waitCredit(id, required) {
  while ((credits.get(id) ?? 0) < required) await new Promise(resolve => creditWaiters.set(id, resolve));
}
async function callWithBytes(method, payload, bytes, epoch, generation) {
  const streamId = nextStreamId; nextStreamId += 2;
  const sha256 = crypto.createHash("sha256").update(bytes).digest("hex");
  sendControl({ wireVersion:1,messageType:"request",requestId:crypto.randomUUID(),method:"stream.open",deadlineMs:0,bindingEpoch:epoch,transportGeneration:generation,payload:{streamId,size:bytes.length,sha256} });
  const response = call(method, {...payload,streamId}, epoch, generation);
  let offset = 0; let sequence = 1;
  while (offset < bytes.length) {
    await waitCredit(streamId, offset + 1);
    const end = Math.min(bytes.length, offset + 1024 * 1024, credits.get(streamId));
    child.stdin.write(frame(BINARY, streamId, sequence++, bytes.subarray(offset, end))); offset = end;
  }
  child.stdin.write(frame(END, streamId, sequence, Buffer.concat([u64(bytes.length), Buffer.from(sha256, "hex")])));
  return response;
}

const started = performance.now();
await call("runtime.hello", { nonce:[...crypto.randomBytes(32)], wireVersions:[1], pluginId:"com.megle.fake-sync", version:"1.0.0" });
await call("describe", null);
const payload = crypto.randomBytes(9 * 1024 * 1024);
const hash = crypto.createHash("sha256").update(payload).digest("hex");
const epoch = crypto.randomUUID();
await callWithBytes("object.put_immutable", {key:"segments/large",sha256:hash}, payload, epoch, 1);
const page = await call("delta.list", {scope:{kind:"library",namespace:"library-0",bindingEpoch:epoch,transportGeneration:1},cursor:null,pageToken:null,limit:1}, epoch, 1);
if (page.objects.length !== 1 || page.objects[0].sha256 !== hash) throw new Error("process provider round trip failed");
const read = await call("object.get", {key:"segments/large",maxBytes:payload.length}, epoch, 1);
const downloaded = incoming.get(read.streamId);
if (!downloaded?.complete || Buffer.concat(downloaded.chunks).compare(payload) !== 0) throw new Error("process provider streamed download failed");
const uploadBytes = Buffer.from("resumable-upload");
const uploadHash = crypto.createHash("sha256").update(uploadBytes).digest("hex");
const upload = await call("upload.begin", {key:"checkpoints/one",size:uploadBytes.length,sha256:uploadHash}, epoch, 1);
await callWithBytes("upload.write", {uploadId:upload.uploadId,offset:0}, uploadBytes.subarray(0, 5), epoch, 1);
await callWithBytes("upload.write", {uploadId:upload.uploadId,offset:5}, uploadBytes.subarray(5), epoch, 1);
await call("upload.commit", {uploadId:upload.uploadId}, epoch, 1);
const authenticated = await call("auth.complete", {flowId:"benchmark",fields:{} });
if (!authenticated.authenticated) throw new Error("process provider auth conformance failed");
const beforeBrowse = providerCalls;
const browseStarted = performance.now();
for (let i = 0; i < 100; i += 1) fs.statSync(path.join(root, "package.json"));
const browseMs = performance.now() - browseStarted;
const browseProviderCalls = providerCalls - beforeBrowse;
const rpcStart = performance.now();
await Promise.all(Array.from({length:8}, (_, index) => { const id = crypto.randomUUID(); return call("delta.list", {scope:{kind:"library",namespace:`library-${index}`,bindingEpoch:id,transportGeneration:1},cursor:null,pageToken:null,limit:100}, id, 1); }));
const elapsedMs = performance.now() - rpcStart;
let privateBytes = null;
if (process.platform === "win32" && child.pid) { const probe = spawnSync("powershell", ["-NoProfile","-Command",`(Get-Process -Id ${child.pid}).PrivateMemorySize64`], {encoding:"utf8"}); privateBytes = Number.parseInt(probe.stdout.trim(),10) || null; }
await call("shutdown", null).catch(() => {}); child.stdin.end(); await new Promise(resolve => child.once("exit", resolve));
const result = { libraries:8, processes:1, framedChild:true, streamedBytes:payload.length, elapsedMs, startupMs:performance.now()-started, browseMs, privateBytes, rpcCalls:providerCalls, browseProviderCalls };
if (elapsedMs > 2000 || browseProviderCalls !== 0 || pending.size !== 0) throw new Error(`plugin runtime benchmark failed: ${JSON.stringify(result)}`);
console.log(`PLUGIN_RUNTIME_BENCH ${JSON.stringify(result)}`);
