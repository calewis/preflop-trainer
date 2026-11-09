#!/bin/sh
# Rebuilds app/trainer.wasm and stamps sw.js with a version derived from the
# app's contents, so an update reaches clients only when something changed.
# Run it from the repo root on a machine with the wasm32-unknown-unknown target,
# and commit app/trainer.wasm and sw.js together.
set -eu
cd "$(dirname "$0")/.."
# Keep local paths out of the panic messages compiled into the wasm.
# When prefixes overlap the last match wins, so the repo root goes last.
RUSTFLAGS="--remap-path-prefix=$HOME=/home --remap-path-prefix=$PWD=/src" \
  cargo build --manifest-path web/Cargo.toml --release --lib --target wasm32-unknown-unknown
cp "${CARGO_TARGET_DIR:-web/target}"/wasm32-unknown-unknown/release/trainer_web.wasm app/trainer.wasm
python3 - <<'PY'
import hashlib, pathlib, re
files = sorted(p for p in pathlib.Path("app").iterdir() if p.is_file())
files += [pathlib.Path(n) for n in ("index.html", "trainer.html", "manifest.webmanifest")]
digest = hashlib.sha256()
for path in files:
    digest.update(path.read_bytes())
sw = pathlib.Path("sw.js")
version = digest.hexdigest()[:12]
sw.write_text(re.sub(r"const VERSION = '[^']*';", f"const VERSION = '{version}';", sw.read_text()))
print("cache version", version)
PY
