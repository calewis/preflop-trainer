"""Renders app/icon-*.png from app/icon.svg with headless Chrome. Run on hal."""
import pathlib, subprocess, sys, tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
svg = (ROOT / "app/icon.svg").read_text()

def render(name, size, padded):
    body = svg
    if padded:
        html = f'<body style="margin:0;background:#1f4e79"><div style="width:{size}px;height:{size}px;padding:{size//10}px;box-sizing:border-box">{body}</div></body>'
    else:
        html = f'<body style="margin:0;background:transparent"><div style="width:{size}px;height:{size}px">{body}</div></body>'
    with tempfile.NamedTemporaryFile("w", suffix=".html", delete=False) as f:
        f.write(html)
    out = ROOT / "app" / name
    subprocess.run(["google-chrome", "--headless=new", "--no-sandbox", "--hide-scrollbars",
                    f"--screenshot={out}", f"--window-size={size},{size}",
                    "--default-background-color=00000000", f"file://{f.name}"],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

render("icon-180.png", 180, True)
render("icon-192.png", 192, False)
render("icon-512.png", 512, False)
render("icon-512-maskable.png", 512, True)
print(sorted(p.name for p in (ROOT / "app").glob("icon-*")))
