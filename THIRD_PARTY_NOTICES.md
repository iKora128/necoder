# Third-party notices

necoder is distributed under AGPL-3.0-or-later. It also incorporates or distributes third-party
components under their own licenses. This file records the components most relevant to the shipped
application and website; `Cargo.lock` and `cargo metadata` are the complete machine-readable Rust
dependency inventory.

## Zed / GPUI revision

Source revision: `zed-industries/zed@b2d9c2e122fbc408d42276b4456243ba4f90f181`.

- `gpui`, `gpui_platform`, `gpui_apple`, `gpui_macos`, `sum_tree`: Apache-2.0 as marked in the
  upstream crate manifests.
- `ztracing`, `ztracing_macro`, `zlog`: GPL-3.0-or-later as marked in the upstream crate manifests.
- Copyright: Zed Industries, Inc. and Zed contributors.
- Source and license texts: <https://github.com/zed-industries/zed/tree/b2d9c2e122fbc408d42276b4456243ba4f90f181>

Copies of the relevant license texts are included at `third_party/licenses/GPL-3.0.txt` and
`third_party/licenses/Apache-2.0.txt` and are shipped with binary distributions.
Tagged binary releases also attach `zed-b2d9c2e122fbc408d42276b4456243ba4f90f181-source.tar.gz`
alongside the binaries so the source for the pinned GPL components remains available from the same
distribution page. necoder's corresponding source is the source archive for that release tag.

The GPL-3.0-or-later components are combined with necoder under the GPLv3/AGPLv3 section 13
compatibility provisions. Their presence means the shipped combination is not described as
"permissive-only" or "GPL-free".

## Rust dependencies

Rust dependencies not listed above remain under the license expressions recorded by their upstream
packages and in `Cargo.lock`. `cargo deny check` audits the configured registry and git sources,
licenses, and advisories, but publish=false packages require the separate license-boundary check in CI.
This notice is a focused distribution notice, not a replacement for that complete dependency inventory.

## Bundled fonts

- IBM Plex Sans JP: SIL Open Font License 1.1. See `assets/fonts/IBMPlexSansJP-OFL.txt`.
- Guguru Sans Code: SIL Open Font License 1.1. See `assets/fonts/GuguruSansCode-OFL.txt`.
- Archivo webfont: SIL Open Font License 1.1. Copyright 2019 Omnibus-Type.
  Source and license: <https://github.com/Omnibus-Type/Archivo>.

## Icons and brand marks

- Lucide icons: ISC. See `crates/necoder/assets/icons/LICENSE.md`.
- Simple Icons glyphs: CC0-1.0. See `crates/necoder/assets/icons/LICENSE.md`.

Product names and logos remain trademarks of their respective owners. Their appearance in necoder is
for identification and interoperability and does not imply sponsorship, endorsement, or affiliation.

## Landing-page runtime

The landing page includes React and React DOM, licensed under the MIT License.

Copyright (c) Meta Platforms, Inc. and affiliates.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction,
including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial
portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT
LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN
NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

React source and license: <https://github.com/facebook/react/tree/v18.3.1>.
