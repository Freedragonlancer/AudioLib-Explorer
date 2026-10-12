# Third-party software

AudioLib Explorer uses Rust components and the Windows audio stack. The portable package includes a dependency/version/license manifest and available license notices in `licenses/`.

| Component | Purpose | License |
| --- | --- | --- |
| Slint 1.17.1 | Desktop interface and software rendering | Tri-licensed: GPL-3.0-only OR Slint Royalty-free License 2.0 OR Slint commercial license. This project uses Slint under GPL-3.0-only |
| rusqlite / SQLite | Library storage and FTS5 | MIT / SQLite public domain |
| CPAL 0.18.2 | WASAPI / ASIO output | Apache-2.0 |
| Symphonia 0.6.1 | Streaming WAV audio decoding | MPL-2.0 |
| roxmltree 0.21.1 | Bounded iXML, aXML and XMP Metadata parsing | MIT OR Apache-2.0 |
| encoding_rs | UTF-16 and legacy text Metadata decoding | Apache-2.0 OR MIT OR BSD-3-Clause |
| RustCrypto blowfish 0.9.1 | Read-only legacy Soundminer SMED compatibility | MIT OR Apache-2.0 |
| Rubato 5.0.1 | Sample-rate conversion | MIT |
| SoundTouch 2.3.2 via soundtouch 0.5.4 / soundtouch-ffi 0.4.1 | Independent audition time stretching and pitch shifting | LGPL-2.1 (as declared by the Rust crates) |
| jieba-rs | Chinese segmentation | MIT |
| ICU4X icu_casemap 2.3.0 | Unicode full case folding for literal tag filters | Unicode-3.0 |
| Steinberg ASIO SDK 2.3.4 | Windows ASIO interface | GPLv3 or proprietary Steinberg agreement; use the SDK LICENSE supplied with the verified archive |

AudioLib Explorer itself is licensed under GPL-3.0-only (see [LICENSE](LICENSE)). All third-party licenses listed here were checked with `cargo metadata`; none is known to conflict with GPL-3.0 (see the compatibility notes below). No audio-interface drivers are redistributed. The Steinberg ASIO SDK is available under GPLv3, which matches this project's license; an ASIO-enabled binary is therefore distributed under GPL-3.0 with the corresponding source, and the SDK's LICENSE from the verified archive is included in the package notices.

Sources: [Slint licensing](https://slint.dev/terms-and-conditions), [CPAL](https://github.com/RustAudio/cpal/tree/v0.18.2), [Symphonia](https://github.com/pdeljanov/Symphonia), [Rubato](https://github.com/HEnquist/rubato), [Steinberg SDK](https://www.steinberg.net/developers/).

The pinned ASIO SDK archive is `ASIO-SDK_2.3.4_2025-10-15.zip`, SHA-256 `D5EBF0C20DD2C5F43771FD0C1418F4B361BF52434EE670097CFA6B3A335E2ECA`, obtained over HTTPS from Steinberg's official download server.

SoundTouch is built from the pinned, bundled source in soundtouch-ffi 0.4.1; no runtime DLL download is needed. Its LGPL text is included in the portable notices. See [upstream](https://codeberg.org/soundtouch/soundtouch) and [Rust bindings](https://github.com/Cyanistic/soundtouch). Public redistribution of a statically linked build must also satisfy the LGPL relinking/source obligations.

Compatibility notes: Apache-2.0, MIT, BSD, ISC, Zlib, Unicode-3.0, CDLA-Permissive-2.0 and MPL-2.0 (Symphonia and others, file-level copyleft) components can be combined with GPL-3.0-only. The SoundTouch crates declare LGPL-2.1; linking it statically into a GPL-3.0 program is permitted when the LGPL section on combined works is satisfied (corresponding source is provided, and users can relink or modify the library). If you redistribute binaries, keep the notices in `licenses/` and the source offer in place.
