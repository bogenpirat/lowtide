"""Render res/lowtide.ico: a battery whose charge is a low tide of water."""
from PIL import Image, ImageDraw
import math
import pathlib

def render(size):
    s = size * 8  # supersample, then downscale for clean edges
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    pad = s * 0.10
    body = (pad, s * 0.22, s - pad - s * 0.08, s * 0.78)
    r = s * 0.08
    # terminal nub
    d.rounded_rectangle((body[2], s * 0.40, body[2] + s * 0.08, s * 0.60), r * 0.4, fill=(235, 240, 245, 255))
    # outline
    d.rounded_rectangle(body, r, fill=(30, 36, 44, 255), outline=(235, 240, 245, 255), width=int(s * 0.06))
    # water: a sine wave filling the lower third
    inset = s * 0.09
    x0, y0, x1, y1 = body[0] + inset, body[1] + inset, body[2] - inset, body[3] - inset
    level = y1 - (y1 - y0) * 0.38
    amp = (y1 - y0) * 0.08
    pts = [(x0, y1)]
    steps = 64
    for i in range(steps + 1):
        x = x0 + (x1 - x0) * i / steps
        pts.append((x, level + amp * math.sin(i / steps * 2 * math.pi * 1.5)))
    pts.append((x1, y1))
    d.polygon(pts, fill=(64, 170, 255, 255))
    return img.resize((size, size), Image.LANCZOS)

out = pathlib.Path(__file__).resolve().parent.parent / "res" / "lowtide.ico"
sizes = [16, 20, 24, 32, 40, 48, 64, 256]
big = render(256)
big.save(out, sizes=[(n, n) for n in sizes], append_images=[render(n) for n in sizes[:-1]])
print("wrote", out)
