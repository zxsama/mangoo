# PPM decoder example

This is a real accountless `file-decoder.v1` process, bundled as
`com.mangoo.ppm-decoder`. It decodes a single `.ppm` P3 (ASCII RGB) or P6 (binary
RGB) image with Maxval 255 through `image`'s PNM decoder, then returns source
pixels resized to the requested bound as tightly packed RGBA8. Core's built-in
extension list deliberately does not include `.ppm`.

P1/P2/P4/P5/P7, other Maxval values, multiple-image sequences and non-whitespace
trailing bytes are unsupported. Truncated samples are invalid. Input is limited
to 64 MiB, header to 64 KiB, source to 16 million pixels and output to the
purpose-specific limits in [the protocol](../../contracts/plugins/file-decoder-v1.md).
No fonts, network, source path access or image viewer UI are needed.

Build/trust registration: run `node tools/build/build-bundled-plugins.mjs`, then
rebuild Core. The normal Host plugin controls grant temporary storage and enable
the decoder without creating a sync account. The template is under
`bundled-plugins/com.mangoo.ppm-decoder`; generated executable, manifest and trust
hashes live under Cargo's target directory/compiled trust registry.

Run `cargo test -p mangoo-ppm-decoder` for content, malformed-input and actual
child-process streaming tests. After building bundles, run
`cargo test -p mangoo-core decoder_` for the Host adapter and lifecycle fence.
The process test includes a 6 MiB P6 input (larger than the 1 MiB window) and
8 KiB slow output credit. These prove transfer behavior; OS working-set peak
and full desktop acceptance are recorded separately rather than inferred from
the successful functional test.
