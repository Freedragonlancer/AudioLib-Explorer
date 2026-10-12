# Application icon

The application mark combines stacked sample-library folders with a five-bar Waveform, using the desktop's charcoal/teal palette. The outside of the rounded tile is transparent. Generated with the built-in ImageGen tool on 2026-10-12; the original image is retained unchanged. Only resizing and container conversion are performed by the authoring utility.

## Assets and integration

All assets live in [`resources/icons`](../resources/icons/):

| File | Use |
| --- | --- |
| `AudioLibExplorer-source.png` | Original generated 1280×1280 RGBA master |
| `AudioLibExplorer-1024.png` | 1024×1024 high-resolution export |
| `AudioLibExplorer.png` | 256×256 embedded Slint window icon and README preview |
| `AudioLibExplorer.ico` | Windows EXE icon: 16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 128, 192, 256 px |
| `AudioLibExplorer.icns` | macOS icon container with standard and Retina representations |
| `AudioLibExplorer.iconset/` | Apple standard 16, 32, 128, 256, 512 point images, each at 1× and 2× |

`app/audiolib-desktop/build.rs` embeds the ICO with winresource and watches both application icon inputs for changes. Slint's `Window.icon` embeds the PNG, so the window icon does not depend on an external runtime file. The existing Windows PerMonitorV2 manifest is retained. Portable packaging already copies `resources/`, including these assets.

The desktop Cargo manifest contains `package.metadata.bundle` with the ICNS path for **cargo-bundle**. On a Mac with native desktop build prerequisites and cargo-bundle available, run from `app/audiolib-desktop` (omit `--bin`, so the package's primary binary and bundle metadata are selected):

```sh
cargo +1.97.1 bundle --release --format osx --target aarch64-apple-darwin
# Use x86_64-apple-darwin on Intel Macs.
```

An explicit native target overrides the repository's Windows target default. Cargo-bundle copies the ICNS into `Contents/Resources`, sets `CFBundleIconFile`, and emits `NSHighResolutionCapable=true`. If using a different bundler, configure those same entries. A standard macOS tool can also regenerate the container:

```sh
iconutil -c icns resources/icons/AudioLibExplorer.iconset -o resources/icons/AudioLibExplorer.icns
```

**Scope:** Windows is the delivered desktop platform. macOS assets and bundle metadata are prepared, but this Windows-machine change does not certify a macOS desktop port, signing/notarization, Dock/Finder appearance or Retina hardware behavior. Do not publish a macOS application without native build and runtime verification.

## Regeneration

Run `./tools/generate-app-icons.ps1` on Windows. It uses .NET System.Drawing, retains alpha, and writes only the named derived icon files. Ordinary Rust builds use the checked-in outputs and need no image-generation service, Python or new Cargo dependency. The source master is not overwritten.

The Windows ICO covers title-bar/taskbar intermediate DPI sizes. ICNS uses PNG chunks `icp4`, `icp5`, `ic07`, `ic08`, `ic09`, `ic10`, `ic11`, `ic12`, `ic13`, `ic14`; Retina chunks retain their logical size and scale even when two entries share the same physical pixel dimensions.

References: [Windows icon sizes and scaling](https://learn.microsoft.com/en-us/windows/apps/design/iconography/app-icon-construction), [Apple iconset naming](https://developer.apple.com/library/archive/documentation/Xcode/Reference/xcode_ref-Asset_Catalog_Format/IconSetType.html), [Slint Window.icon](https://docs.slint.dev/latest/docs/slint/reference/window/window/#icon), [cargo-bundle](https://github.com/burtonageo/cargo-bundle).

## Generation prompt

Built-in ImageGen, `transparent_background=true`:

> Use case: logo-brand. Asset type: production desktop application icon for AudioLib Explorer, an audio sample library manager, Windows and macOS high DPI. Primary request: a modern flat icon combining a neatly stacked audio-library folder / archive cards with one bold centered waveform made from five rounded vertical bars. Single iconic mark, strong instantly readable silhouette, optically centered on a charcoal rounded-square tile. Use the existing application's charcoal and restrained teal palette: dark charcoal #20242a tile, clear light teal #8fcfc3 foreground folder and dark negative-space waveform. Flat geometric filled shapes, clean edges, generous spacing, excellent legibility down to 16px. Square 1024x1024 image, entire rounded-square tile occupies about 86 percent of canvas width with equal transparent margins, genuine alpha transparency outside the tile. No words, no letters, no musical note, no headphones, no decorative tiny details, no 3D, no perspective, no gradients, no shadows, no border, no texture, no watermark. Render only one finished app icon, not a presentation or multiple variants.

The tool returned 1280×1280 pixels; the 1024×1024 export and platform sizes are derived from that original.
