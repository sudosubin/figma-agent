# figma-agent

Local font helper for [Figma](https://www.figma.com), Linux and macOS.

A small Rust daemon that exposes the two endpoints the Figma web client
calls to enumerate and load locally-installed fonts.

| Endpoint | Purpose |
|---|---|
| `GET /font-files` | List installed fonts + metadata (mirrors macOS Figma Agent's schema) |
| `GET /font-file?file=<path>` | Stream a font file by path |

`OPTIONS` preflight is handled for CORS, including
`Access-Control-Allow-Private-Network: true`, required by Chrome 94+ when
figma.com (public origin) reaches `127.0.0.1` (private network).

Responses are gzip/deflate/zstd-compressed when the client sends
`Accept-Encoding`. `Server: FigmaAgent/<version>` is set on every response.
Trailing slashes are normalised (so `/font-files/` and `/font-files` route
to the same handler). Matches the orig macOS Figma Agent's middleware stack.

The non-browser endpoints from the official macOS Figma Agent (spell-check,
`desktop/open-url`, `clear-data`, `assets/*`, `font-preview`) are
intentionally **not** implemented; they exist for the Figma desktop app's
IPC with macOS system APIs and are not exercised by figma.com itself.

## Build

```bash
cargo build --release
```

`--release --no-default-features` to drop TLS (smaller binary, HTTP-only).

## Run

```bash
./target/release/figma-agent
```

Default ports: HTTP `127.0.0.1:44950`, HTTPS `127.0.0.1:44951` (same as the
macOS Figma Agent).

## Config

Optional JSON file at `$XDG_CONFIG_HOME/figma-agent/config.json`,
falling back to `~/.config/figma-agent/config.json` on both Linux and macOS.

```json
{
  "host": "127.0.0.1",
  "port": 44950,
  "tls_port": 44951,
  "tls_cert": null,
  "tls_key": null,
  "font_dirs": [
    "/usr/share/fonts",
    { "path": "/Users/me/Library/Fonts", "user_installed": true }
  ]
}
```

- `font_dirs` entries can be plain strings (treated as `user_installed=false`)
  or detailed objects to override the classification.
- `tls_port: null` disables HTTPS entirely.
- If `tls_cert`/`tls_key` are absent, a self-signed cert is generated for
  `localhost`/`127.0.0.1` at startup; trust it once on your machine
  (macOS: `security add-trusted-cert`; Linux: NSS DB).

## Font discovery

Two sources, deduped by absolute path:

1. **OS registry**: `CTFontManagerCopyAvailableFontURLs` on macOS, `fc-list`
   on Linux. Captures Font Book registrations, Adobe Fonts, fontconfig user
   dirs, and anything else the system already knows about.
2. **`font_dirs` config**: explicit directories you want walked. Entries
   here override the auto-classification (e.g., mark a system path as
   `user_installed: true` if you want).

The parsed catalogue is cached to
`$XDG_CACHE_HOME/figma-agent/font_cache.json` (default
`~/.cache/figma-agent/font_cache.json`) on Linux and macOS alike. Cache is
keyed by daemon version; delete the file or upgrade to force a refresh
(e.g., after adding fonts).

## Response schema

```jsonc
{
  "version": "0.1.0",
  "modified_at": 1716000000,
  "modified_fonts": [],
  "fonts": [
    {
      "family": "Inter",
      "style": "Regular",
      "postscript": "Inter-Regular",
      "weight": 400.0,
      "stretch": 100.0,
      "italic": false,
      "variationAxes": [
        {
          "tag": "wght",
          "name": "Weight",
          "value": 400.0,
          "min": 100.0,
          "max": 900.0,
          "default": 400.0,
          "hidden": false
        }
      ],
      "user_installed": false,
      "name": "Inter Regular",
      "path": "/usr/share/fonts/Inter-Regular.ttf"
    }
  ],
  "request_id": 1,
  "elapsed_ms": 42
}
```

This matches the macOS Figma Agent's `FontInfo` schema 1:1, except for the
trailing `path` field, which we add so the follow-up `GET /font-file` call
has a usable identifier (the orig daemon's client knows the path through
other means).

Variable fonts with `fvar` named-instances expand to one entry per instance
(Inter VF becomes Thin, Light, Regular, etc.), matching CoreText's behaviour
on the orig macOS agent. The instance's `weight` / `stretch` reflect the
runtime `wght` / `wdth` axis coordinates, and each `variationAxes[i].value`
is the per-instance coordinate rather than the file-level default.

`/font-file` errors return JSON: `{ error, detail, version, request_id }`.
Validation: non-empty, ≤ 4 KB path, absolute, no `..` segments, regular
file, ≤ 256 MB, and inside one of `font_dirs`. The 256 MB ceiling is higher
than orig's 32 MB so large macOS CJK collections (`AppleSDGothicNeo.ttc`,
`PingFang.ttc`) remain reachable.

## License

MIT, see [LICENSE](./LICENSE).
