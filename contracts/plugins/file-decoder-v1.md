# File decoder v1

`file-decoder.v1` is a local, accountless process capability. It is separate
from `sync-provider.v1` and from the plugin UI SDK. Version 1 accepts only one
capability per manifest; there is no Provider account, credential broker,
network permission, plugin HTML or original-file path in a decoder request.

The closed `rpc.decoder` declaration contains `implementationVersion` (semver),
`extensions` (1–64 unique lowercase alphanumeric suffixes, 1–16 characters,
without the dot), and unique `purposes` (`thumbnail`, `preview`). The exact
method map is `health`, `diagnostics`, `shutdown`, `decoder.decode`, all version
1. Activation events are `file-decode-requested` and `diagnostics-requested`.
The only accepted requested permission is bounded `temp-storage`.
See [the complete example](fixtures/valid-file-decoder-v2.json).

The accountless `runtime.hello` payload has `nonce`, `wireVersions`, `pluginId`,
`version`, `capabilities`, `grantedPermissions`, `methodVersions`, `limits`.
The acknowledgement returns the same identity, nonce, capabilities, methods
and limits, with `wireVersion: 1` replacing `wireVersions`. No `accountUuid`
field is present. Existing Provider hello and its complete method map are unchanged.

All messages use existing wire-v1 framing (17-byte little-endian header,
bounded JSON control and binary credit streams). `stream.open` remains exactly
`{streamId,size,sha256}`. Host input IDs are odd, plugin output IDs even. Hashes
are lowercase hexadecimal SHA-256. Credit is cumulative and may never regress
or exceed declared length. Binary and End sequence numbers start at 1 and
increase once per frame. End carries little-endian u64 length and 32 hash bytes.

Core snapshots an already-authorized source into an anonymous owned temporary
file with a fixed 256 KiB buffer, verifies source metadata before/after copying,
and hashes the exact snapshot. Source/root/Library/attempt validation remains
required at the cache transaction. Neither input nor output paths reach the
plugin or renderer. The plugin stages input in its own anonymous temporary
file; OS file ownership cleans interrupted/crashed sessions without a broad
directory deletion. All input/output files are closed after the operation.

`decoder.decode` request:

```json
{
  "jobId": "01991234-1234-7123-8123-123456789abc",
  "streamId": 1,
  "purpose": "thumbnail",
  "size": 123,
  "sha256": "<64 lowercase hexadecimal characters>",
  "maxEdge": 1024,
  "maxOutputPixels": 10000000,
  "maxSourcePixels": 16000000,
  "maxHeaderBytes": 65536
}
```

The request follows input stream.open and precedes its binary data. The plugin
must continue consuming input/returning credit before decoding. Once the complete
input length/hash is verified, it generates one tightly packed RGBA8 output.
The output stream.open and response may precede output End; Core retains the
single session operation until input, response, complete output and raster
validation finish. Extra or unsolicited output streams are rejected before
creating a temporary file.

```json
{
  "jobId": "01991234-1234-7123-8123-123456789abc",
  "streamId": 2,
  "width": 320,
  "height": 240,
  "format": "rgba8",
  "size": 307200,
  "sha256": "<64 lowercase hexadecimal characters>"
}
```

Core verifies job/stream association, dimensions, multiplication overflow,
`width * height * 4`, actual length and hash before encoding/publishing. Errors
use ordinary wire error envelopes: `unsupported`, `invalid_data`,
`resource_limit`, all `retryable: false` in this example. Crash, protocol error
or timeout retires the process; automatic repeated respawn is suppressed until
an explicit lifecycle change. Ordinary caller cancellation or a dropped request
owner retires and joins its process without latching a decoder fault. If the
plugin remains enabled in the same epoch, the supervisor may restore capacity
after that join; disable/restart still fences the old epoch. No-match is an
unavailable presentation rather than a failed/missing file.

Budgets: 64 MiB input; thumbnail/preview longest edge 1024/3072; at most 10 MP
or 40,000,000 output bytes; at most 16 MP source pixels; 64 KiB header/control;
256 KiB chunks; 1 MiB receive window; at most four queued frames; one operation
per process; two active global jobs and 32 admitted/queued requests. Thumbnail
and preview use one 15/30-second deadline from admission through validation.
Two 64 MiB input snapshots plus one output fit inside 256 MiB per-task temporary
budget and 768 MiB global budget. Windows decoder jobs additionally enforce
256 MiB process committed memory and one child process; Unix sets a 256 MiB
address-space limit. These OS limits differ from measured working-set usage.

`DecoderPublicationGuard` outlives RPC ownership until the atomic cache commit.
`publish` arbitrates against the manager's closed epoch under the same lock.
Disable first closes admission, cancels work, joins file workers/streams/child
and publication owners, then acknowledges completion. Cache identity includes
plugin/version/implementation/artifact hash. Successful images exclude the
enabled epoch, so a verified unchanged-source image survives disable. Negative
entries additionally include the lifecycle epoch: explicit retry/re-enable can
recover a failed process without discarding valid pictures. Queue saturation and
caller cancellation do not persist a permanent content failure. Source hash
changes invalidate generic presentations even when size/coarse mtime are unchanged.
Failure provenance uses the decoder epoch that actually executed the job, not
the epoch when a request first entered the queue. A queued request that survives
a restart cannot publish a failure under the previous decoder identity.

The global two-job permit outlives RPC and remains held through WebP encoding
and cache publication; completed raster buffers cannot accumulate outside the
quota. Thumbnail workers use the existing source/attempt transaction on the
single writer, with `source_dimensions=None`. Preview uses the same decoder
owner and source checks with a `preview_3072` cache entry; unavailable content
falls back to an existing validated thumbnail. The Web image lane requests this
authenticated static representation and never receives arbitrary original bytes.
