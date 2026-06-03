![Lama Blanket icon](icon.webp) 
# Lama Blanket

Lama Blanket is a desktop wrapper around [`llama.cpp`](https://github.com/ggml-org/llama.cpp)'s `llama-server`, built in Rust with [`egui`](https://github.com/emilk/egui).

It is aimed at local model serving with a simple GUI for picking GGUF models, estimating reasonable launch settings, saving presets, and managing the server without having to remember terminal commands.

## Features

- GGUF model inspection and metadata parsing
- Hardware detection for CPU, RAM, and GPU-aware defaults
- Automatic launch argument generation for `llama-server`
- Resource estimates for RAM, VRAM, KV cache, and GPU layer offload
- Saved presets with JSON import support
- Automatic `mmproj` detection for multimodal models
- Light and dark theme

## Current Scope

The project is currently stable and usable, only tested on Windows.\
Some functionality, such as startup registration and the current tray behavior work, is Windows-focused.

## How to run

1. Download the `llama.cpp` binaries from the [`llama.cpp` repository](https://github.com/ggml-org/llama.cpp).
2. Put the `llama.cpp` folder next to `lama-blanket.exe`, or put `lama-blanket.exe` inside your `llama.cpp` folder.
3. Run `lama-blanket.exe`.

## How `llama-server` Is Found

Lama Blanket looks for the server executable in this order:

1. `llama.cpp/llama-server.exe`
2. `../llama.cpp/llama-server.exe`
3. `llama-server.exe`

If you keep a local `llama.cpp` checkout beside this app, the first two options are the expected layout.

## Build

To build Lama Blanket you need:

- Rust toolchain
- Cargo

```powershell
cargo build
```

For a release build:

```powershell
cargo build --release
```

## How Presets Are Estimated

The app uses heuristics from model metadata and detected hardware:

- Context size starts from the GGUF context length, capped at `128000`, or falls back to `4096`.
- GPU layers are estimated from available VRAM after subtracting a safety overhead and estimated KV cache usage.
- Threads default to physical CPU cores.
- Batch threads default to logical CPU cores.
- Flash attention defaults to `auto` for NVIDIA-backed `llama` models and `off` otherwise.
- `mlock` defaults on when available RAM is comfortably above model size.

These are starting points, not guaranteed best values. Larger context sizes, aggressive offload, and quantization choices can still require manual tuning.

## Presets

Presets are stored in JSON and persist across launches.

- `Save` stores the current configuration as a named preset.
- `Import...` imports one preset or an array of presets from a JSON file.
- Imported presets are merged into the local preset list and remain available until deleted.
- If an imported preset has the same name as an existing preset, it replaces it.


## Development

Useful commands:

```powershell
cargo check
cargo test
```

## License

MIT. See [LICENSE](LICENSE).