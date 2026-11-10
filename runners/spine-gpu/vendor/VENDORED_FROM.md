# Vendored: bevy_spine_gpu

`vendor/bevy-spine-gpu/` is a copy of the `bevy_spine_gpu` crate from the bevy-spine-gpu repository (16278bdfa17c0d2930c54936b289711fa01a8c43; `Cargo.toml`, `COPYRIGHT`, `LICENSE-*`, `README.md` and `src/`, without its `assets/` and `examples/`: the runner
uses this repository's rigs). It is unmodified. The runner depends on it by path and uses Bevy 0.14 with default
features (Bevy 0.14.2, wgpu 0.20.1 in `Cargo.lock`).
