# peak-meter-way

peak-meter-way is a Waybar module that shows an audio peak meter for PipeWire. By default it meters the default input, for example a microphone. With `--monitor` it meters the default output instead.

The meter is a horizontal bar with three color zones and a peak-hold tick:

```
━━━━━━━━━━━━━╸━━━┃━━
```

It uses little CPU and prints nothing while the audio is silent.

## Requirements

- Waybar
- PipeWire
- A font that has box-drawing characters (`━ ╸ ┃`)

## Install

### Nix

To test peak-meter-way without an installation, run it with `nix run`. It prints one JSON line for each update.

```sh
nix run github:mostlymaxi/peak-meter-way-rs
```

To install peak-meter-way into your user profile:

```sh
nix profile add github:mostlymaxi/peak-meter-way-rs
```

Then set `"exec": "peak-meter-way"` in the Waybar configuration. Older Nix versions call this command `nix profile install`.

To use peak-meter-way in a NixOS or Home Manager flake:

1. Add the flake to the inputs of your flake:

   ```nix
   peak-meter-way.url = "github:mostlymaxi/peak-meter-way-rs";
   peak-meter-way.inputs.nixpkgs.follows = "nixpkgs";
   ```

2. Use the package in the `exec` field of the Waybar module:

   ```nix
   exec = lib.getExe inputs.peak-meter-way.packages.${pkgs.stdenv.hostPlatform.system}.default;
   ```

The flake also has an overlay that adds `pkgs.peak-meter-way`:

```nix
nixpkgs.overlays = [ inputs.peak-meter-way.overlays.default ];
```

### Cargo

1. Install the PipeWire development files, pkg-config, and clang with your package manager.
2. Install peak-meter-way:

   ```sh
   cargo install --git https://github.com/mostlymaxi/peak-meter-way-rs
   ```

## Waybar configuration

```jsonc
"custom/peak-meter": {
    "format": "{}",
    "escape": false,
    "return-type": "json",
    "exec": "peak-meter-way",
    "restart-interval": 2
}
```

`escape` must be `false`, because the meter uses Pango markup for its colors. `restart-interval` starts the module again if PipeWire restarts.

## Usage

```
peak-meter-way [OPTIONS] [TARGET]
```

| Option | Description |
|---|---|
| `-m`, `--monitor` | Meter the default output instead of the default input. |
| `-t`, `--target <name>` | Meter a specific PipeWire node, by name or serial. |
| `-s`, `--style <style>` | `line`, `blocks`, or `dots`. The default is `line`. |
| `-w`, `--width <n>` | Number of characters in the meter. The default is 20. |
| `-f`, `--fps <n>` | Maximum frame rate. The default is 30. |
| `--floor <dB>` | Lowest level on the scale. The default is -60. |
| `--warn <dB>` | Start of the warn zone. The default is -18. |
| `--hot <dB>` | Start of the hot zone. The default is -6. |
| `--release <dB/s>` | How fast the bar falls. The default is 24. |
| `--hold <ms>` | How long the peak-hold tick stays. 0 disables it. The default is 1200. |
| `--hold-release <dB/s>` | How fast the peak-hold tick falls. The default is 30. |
| `--color-low <hex>` | Color of the normal zone. The default is `#a6e3a1`. |
| `--color-warn <hex>` | Color of the warn zone. The default is `#f9e2af`. |
| `--color-hot <hex>` | Color of the hot zone. The default is `#f38ba8`. |
| `--track-alpha <pct>` | Opacity of the unlit part of the bar, from 0 to 100. The default is 22. |
| `--hide-idle` | Show empty text while the audio is silent, so Waybar hides the module. |
| `-h`, `--help` | Show the help text. |

The module sets the Waybar CSS class to `silent`, `safe`, `warn`, or `peak`. The tooltip shows the peak level in dBFS.

## License

MIT. See [LICENSE](LICENSE).
