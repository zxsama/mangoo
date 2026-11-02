# Fluent System Icons identity subset

Source: https://github.com/microsoft/fluentui-system-icons
Pinned commit: `9eb184e96268d3293462f9d9d19225b4c7b07774`.

`identityFluentPaths.ts` contains a bounded set of original 24px filled/regular SVG path data. Each entry's `source` in `identityCatalog.ts` identifies the upstream `assets/<source>/SVG/ic_fluent_<lowercase_source_with_underscores>_24_{filled,regular}.svg` pair. The path geometry and fill rules are retained; the original solid fill is replaced with `currentColor` by the renderer. Source SVGs have no IDs, masks, gradients or external references.

The collection contains 384 stable identities: the original 66 entries, the next 78 additions, 112 further source pairs, and 128 additional distinct pairs from this same pinned commit. All earlier keys, ordering, labels, keywords, categories and path geometry are preserved in the latest expansion. Fifteen presentation categories organize the choices without changing persisted identity keys. New entries retain the upstream geometry and fill rules, with no substitute paths or runtime asset requests. The picker scales these 24px viewboxes to 18px; existing Library and folder display sizes are independent.

The dedicated numbers category includes all ten official `Number Circle 0` through `Number Circle 9` filled/regular pairs, with no font, text or synthesized-digit fallback. Fourteen additional entries cover number rows, hash signs, numbered lists, page/week numbers, numeric clipboard content, uppercase/lowercase Roman and alphabetic lists, counting, steps and number formatting. These are actual upstream symbols rather than relabeled lookalikes. The other 104 additions extend every existing category. Directory spelling is significant: `IoT`, `SIM`, `USB`, `LTR` and `Abc` match the official source directories. Original monochrome fills (including `#212121` and `#242424`) become `currentColor`; all path geometry remains unchanged.

`identityArtwork.tsx` adds local translucent object faces/page details for the Library identity variant. `folder-heart` composes the official Folder and Heart; `aperture` adds shutter divisions inside the official Record ring. These are Mangoo presentation adaptations, not Microsoft product launch icons or trademarks. Ordinary action/navigation icons continue using the existing project icon system.

MIT License

Copyright (c) 2020 Microsoft Corporation

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

The default folder's open state additionally uses `assets/Folder Open/SVG/ic_fluent_folder_open_24_filled.svg` from the same pinned Microsoft Fluent System Icons commit, `9eb184e96268d3293462f9d9d19225b4c7b07774`. It is a presentation state, not an additional persisted picker key.
