from pathlib import Path
from typing import Iterable, Sequence, Tuple

from PIL import Image


def save_pixels(
    path: Path,
    size: Tuple[int, int],
    pixels: Iterable[Sequence[int]],
    mode: str = "RGB",
) -> Path:
    image = Image.new(mode, size)
    image.putdata(list(pixels))
    image.save(str(path))
    image.close()
    return path

