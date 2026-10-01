from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

from PIL import Image, UnidentifiedImageError

from .errors import RenderDebugError


@dataclass
class LoadedImage:
    path: str
    image: Image.Image
    source_mode: str
    working_mode: str
    channel_names: Sequence[str]
    data_type: str
    channel_max: Optional[float]
    metadata: Dict[str, Any]
    warnings: List[str]

    @property
    def width(self) -> int:
        return self.image.width

    @property
    def height(self) -> int:
        return self.image.height

    def pixel(self, x: int, y: int) -> Tuple[float, ...]:
        value = self.image.getpixel((x, y))
        if isinstance(value, tuple):
            return tuple(value)
        return (value,)

    def info_dict(self) -> Dict[str, Any]:
        return {
            "path": self.path,
            "dimensions": {"width": self.width, "height": self.height},
            "source_mode": self.source_mode,
            "working_mode": self.working_mode,
            "channels": list(self.channel_names),
            "data_type": self.data_type,
            "channel_max": self.channel_max,
            "metadata": self.metadata,
        }


def _mode_spec(mode: str) -> Tuple[Sequence[str], str, Optional[float]]:
    specs = {
        "L": (("gray",), "uint8", 255.0),
        "LA": (("gray", "a"), "uint8", 255.0),
        "RGB": (("r", "g", "b"), "uint8", 255.0),
        "RGBA": (("r", "g", "b", "a"), "uint8", 255.0),
        "CMYK": (("c", "m", "y", "k"), "uint8", 255.0),
        "I": (("value",), "int32", None),
        "F": (("value",), "float32", None),
        "I;16": (("value",), "uint16", 65535.0),
        "I;16L": (("value",), "uint16", 65535.0),
        "I;16B": (("value",), "uint16", 65535.0),
    }
    return specs[mode]


def load_image(path_text: str) -> LoadedImage:
    path = Path(path_text).expanduser()
    if not path.is_file():
        raise RenderDebugError(
            "image_not_found", "Image file does not exist", {"path": path_text}
        )

    warnings: List[str] = []
    try:
        with Image.open(str(path)) as source:
            source.load()
            source_mode = source.mode
            info = dict(source.info)
            if source.mode == "P":
                target = "RGBA" if "transparency" in info else "RGB"
                image = source.convert(target)
                warnings.append("Palette image converted from P to %s" % target)
            elif source.mode == "1":
                image = source.convert("L")
                warnings.append("1-bit image converted from 1 to L (0 or 255)")
            elif source.mode in {"L", "LA", "RGB", "RGBA", "CMYK", "I", "F", "I;16", "I;16L", "I;16B"}:
                image = source.copy()
            else:
                image = source.convert("RGB")
                warnings.append("Unsupported native mode %s converted to RGB" % source.mode)
    except (UnidentifiedImageError, OSError, ValueError) as exc:
        raise RenderDebugError(
            "unreadable_image",
            "Image could not be decoded",
            {"path": path_text, "reason": str(exc)},
        )

    channels, data_type, channel_max = _mode_spec(image.mode)
    metadata: Dict[str, Any] = {
        "icc_profile_present": bool(info.get("icc_profile")),
    }
    if "gamma" in info:
        metadata["gamma"] = info["gamma"]
    return LoadedImage(
        path=str(path.resolve()),
        image=image,
        source_mode=source_mode,
        working_mode=image.mode,
        channel_names=channels,
        data_type=data_type,
        channel_max=channel_max,
        metadata=metadata,
        warnings=warnings,
    )


def load_images(paths: Sequence[str]) -> List[LoadedImage]:
    loaded: List[LoadedImage] = []
    try:
        for path in paths:
            loaded.append(load_image(path))
        return loaded
    except Exception:
        for item in loaded:
            item.image.close()
        raise
