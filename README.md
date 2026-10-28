# qr_creator

Rust QR code **image** generator with the exact rounded-module styling of the
Nimiq demo.

- Origin / styling reference: <https://nimiq.github.io/qr-creator/demo/>
- JS source ported: [`qr-creator`](https://github.com/nimiq/qr-creator)
  (`qr-creator.es6.min.js` canvas renderer — rounded outer corners on dark
  modules + concave gap-fillers on light cells, single `fill()` pass).

It encodes the matrix (via the `qrcode` crate) and rasterizes the same
artwork the JS demo draws on `<canvas>`: dark modules as rounded blobs joined
to neighbours, plus small fillets on light cells so connected corners look
smooth. Output is a real image (`image::RgbaImage` / PNG bytes), not just a
boolean matrix.

## Params

Mirrors the demo JSON. `radius`/`size` accept numbers or numeric strings,
`background: null` means transparent:

```json
{
  "text": "Hello world of QR codes!",
  "radius": "0.5",
  "ecLevel": "H",
  "fill": "#0064a8",
  "background": null,
  "size": "250"
}
```

| field | type | default | notes |
|---|---|---|---|
| `text` | string | `"no text"` | payload, byte mode |
| `radius` | `0.0–0.5` / string | `0.5` | fraction of module size, `floor(radius*module)` px |
| `ecLevel` | `"L" "M" "Q" "H"` | `"L"` | error correction |
| `fill` | `"#rrggbb"` etc. | `"#000"` | `#rgb #rgba #rrggbb #rrggbbaa` |
| `background` | color or `null` | `null` | transparent when null |
| `size` | int / string | `200` | output `size × size` px |
| `quiet` | int | `0` | quiet-zone modules (demo uses 0) |

## Use

```rust
let opts: qr_creator::Options = serde_json::from_str(json)?;
let img: image::RgbaImage = opts.render_image()?;
img.save("qr.png")?;

let png_bytes: Vec<u8> = opts.render_png()?;
```

```sh
cargo run --example demo  # renders the JSON above to demo_qr.png
```

### From GitHub (no crates.io needed)

```toml
[dependencies]
qr_creator = { git = "https://github.com/calumk/calumk-qr-creator" }
```

Pin a version if you like:

```toml
[dependencies]
qr_creator = { git = "https://github.com/calumk/calumk-qr-creator", tag = "v0.1.0" }
```

Versioned `cargo install` / `cargo add qr_creator` without a git URL would
require publishing to [crates.io](https://crates.io) (`cargo publish`); that
is optional — git dependencies work directly from this repo.

## Styling fidelity

- Module width `size / (width + 2*quiet)`, edges `round()`ed like the demo.
- Dark cell corners rounded only where both adjacent neighbours are light
  (`!N && !W`, …), radius `floor(clamp(radius,0,0.5) * module)`.
- Light cells add concave quarter-circle gap fillers where `N && W && NW`
  (etc.), same 4 `arcTo` cases as the JS source.
- Single-path `fill()` with antialiasing (`tiny-skia`), background filled first.
- Mask/encoding follows the QR spec (same penalty rules as the JS lib), so
  output decodes with any standard reader (tested with `rqrr`).
