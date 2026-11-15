"""Ikona aplikacji: zaokrąglony kwadrat z gradientem fiolet → róż i białym „T”."""
import sys
from PIL import Image, ImageDraw, ImageFont

S = 1024
out = sys.argv[1]
grad = Image.new("RGBA", (S, S))
px = grad.load()
a, b = (139, 92, 246), (236, 72, 153)
for y in range(S):
    for x in range(S):
        t = (x + y) / (2 * (S - 1))
        px[x, y] = tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3)) + (255,)
mask = Image.new("L", (S, S), 0)
m = 100  # margines jak w ikonach macOS
ImageDraw.Draw(mask).rounded_rectangle((m, m, S - m, S - m), radius=185, fill=255)
icon = Image.new("RGBA", (S, S), (0, 0, 0, 0))
icon.paste(grad, (0, 0), mask)
d = ImageDraw.Draw(icon)
font = ImageFont.truetype("/System/Library/Fonts/Supplemental/Arial Black.ttf", 520)
d.text((S / 2, S / 2 + 10), "T", font=font, fill="white", anchor="mm")
# Mały pasek „napisów” pod literą.
d.rounded_rectangle((330, 760, 694, 800), radius=20, fill=(255, 214, 0, 255))
icon.save(out)
