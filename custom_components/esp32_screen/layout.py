"""Layout model validation for ESP32 Screen (see docs/protocol.md, section 2)."""

from __future__ import annotations

import re
from typing import Any

from .const import (
    KIND_CLIMATE,
    KIND_DOMAINS,
    KIND_LIGHT,
    LIGHT_ICONS,
    LAYOUT_VERSION,
    MAX_COLS,
    MAX_ID_LEN,
    MAX_LABEL_LEN,
    MAX_PAGES,
    MAX_ROWS,
    MAX_TITLE_LEN,
    PAGE_ICONS,
    SENSOR_DOMAIN,
    SENSOR_FIELDS,
)

_ID_RE = re.compile(r"^[A-Za-z0-9_-]+$")
ENTITY_ID_RE = re.compile(r"^[a-z0-9_]+\.[a-z0-9_]+$")
_ENTITY_ID_RE = ENTITY_ID_RE


class LayoutError(ValueError):
    """Raised with a human readable message when a layout is invalid."""


def empty_layout() -> dict[str, Any]:
    """Return the layout sent for screens without one."""
    return {"version": LAYOUT_VERSION, "pages": []}


def _int(value: Any, what: str, low: int, high: int) -> int:
    # bool is an int subclass; JSON true/false is never a valid coordinate.
    if isinstance(value, bool) or not isinstance(value, int):
        if isinstance(value, float) and value.is_integer():
            value = int(value)
        else:
            raise LayoutError(f"{what} must be a whole number")
    if not low <= value <= high:
        raise LayoutError(f"{what} must be between {low} and {high} (got {value})")
    return value


def _str(value: Any, what: str, max_len: int, *, allow_empty: bool) -> str:
    if not isinstance(value, str):
        raise LayoutError(f"{what} must be text")
    value = value.strip()
    if not value and not allow_empty:
        raise LayoutError(f"{what} must not be empty")
    if len(value) > max_len:
        raise LayoutError(f"{what} is longer than {max_len} characters")
    return value


def _ident(value: Any, what: str) -> str:
    value = _str(value, what, MAX_ID_LEN, allow_empty=False)
    if not _ID_RE.match(value):
        raise LayoutError(f"{what} may only contain letters, digits, '_' and '-'")
    return value


def _require_dict(value: Any, what: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise LayoutError(f"{what} must be an object")
    return value


def validate_layout(raw: Any) -> dict[str, Any]:
    """Validate a layout and return a normalized copy.

    Unknown keys are dropped, strings are trimmed. Raises LayoutError.
    """
    layout = _require_dict(raw, "Layout")
    version = layout.get("version", LAYOUT_VERSION)
    if version != LAYOUT_VERSION:
        raise LayoutError(f"Unsupported layout version {version!r}")

    pages_raw = layout.get("pages")
    if not isinstance(pages_raw, list):
        raise LayoutError("Layout needs a list of pages")
    if len(pages_raw) > MAX_PAGES:
        raise LayoutError(f"A screen can have at most {MAX_PAGES} pages")

    page_ids: set[str] = set()
    tile_ids: dict[str, str] = {}
    pages: list[dict[str, Any]] = []

    for page_index, page_raw in enumerate(pages_raw, start=1):
        page = _require_dict(page_raw, f"Page {page_index}")
        title = _str(
            page.get("title"), f"Title of page {page_index}", MAX_TITLE_LEN,
            allow_empty=False,
        )
        where = f'Page "{title}"'
        page_id = _ident(page.get("id"), f"{where}: id")
        if page_id in page_ids:
            raise LayoutError(f'{where}: page id "{page_id}" is used twice')
        page_ids.add(page_id)
        icon = page.get("icon", "")
        if icon is None:
            icon = ""
        if not isinstance(icon, str) or (icon and icon not in PAGE_ICONS):
            raise LayoutError(
                f"{where}: icon {icon!r} is not one the screen can show "
                "(use \"\" or one of the icons in the editor's picker)"
            )
        cols = _int(page.get("cols"), f"{where}: columns", 1, MAX_COLS)
        rows = _int(page.get("rows"), f"{where}: rows", 1, MAX_ROWS)

        tiles_raw = page.get("tiles", [])
        if not isinstance(tiles_raw, list):
            raise LayoutError(f"{where}: tiles must be a list")
        if len(tiles_raw) > cols * rows:
            raise LayoutError(
                f"{where}: {len(tiles_raw)} tiles do not fit in {cols}x{rows} cells"
            )

        grid: list[list[str | None]] = [[None] * cols for _ in range(rows)]
        tiles: list[dict[str, Any]] = []
        for tile_index, tile_raw in enumerate(tiles_raw, start=1):
            tile = _require_dict(tile_raw, f"{where}: tile {tile_index}")
            tile_id = _ident(tile.get("id"), f"{where}: tile {tile_index} id")
            twhere = f'{where}: tile "{tile_id}"'
            if tile_id in tile_ids:
                raise LayoutError(
                    f'{twhere}: tile id is already used on page "{tile_ids[tile_id]}"'
                )
            tile_ids[tile_id] = title

            kind = tile.get("kind")
            if kind not in KIND_DOMAINS:
                raise LayoutError(
                    f"{twhere}: kind must be one of {', '.join(KIND_DOMAINS)}"
                )
            entity_id = tile.get("entity_id")
            if not isinstance(entity_id, str) or not _ENTITY_ID_RE.match(entity_id):
                raise LayoutError(f"{twhere}: pick an entity")
            domain = entity_id.split(".", 1)[0]
            if domain not in KIND_DOMAINS[kind]:
                raise LayoutError(
                    f"{twhere}: a {kind} tile cannot use {entity_id} "
                    f"(allowed: {', '.join(KIND_DOMAINS[kind])})"
                )
            label = _str(
                tile.get("label", ""), f"{twhere}: label", MAX_LABEL_LEN,
                allow_empty=True,
            )
            sensors: dict[str, str] = {}
            for fld in SENSOR_FIELDS:
                value = tile.get(fld, "")
                if value is None:
                    value = ""
                if not isinstance(value, str):
                    raise LayoutError(f"{twhere}: {fld} must be text")
                value = value.strip()
                if value and kind != KIND_CLIMATE:
                    raise LayoutError(
                        f"{twhere}: {fld} is only allowed on climate tiles"
                    )
                if value and (
                    not _ENTITY_ID_RE.match(value)
                    or value.split(".", 1)[0] != SENSOR_DOMAIN
                ):
                    raise LayoutError(
                        f"{twhere}: {fld} must be a sensor.* entity (got {value!r})"
                    )
                sensors[fld] = value
            tile_icon = tile.get("icon", "")
            if tile_icon is None:
                tile_icon = ""
            if not isinstance(tile_icon, str) or (
                tile_icon and (kind != KIND_LIGHT or tile_icon not in LIGHT_ICONS)
            ):
                raise LayoutError(
                    f"{twhere}: icon {tile_icon!r} is not one the screen can show "
                    "(light tiles only: \"\" or one of the icons in the editor's picker)"
                )
            x = _int(tile.get("x"), f"{twhere}: x", 0, cols - 1)
            y = _int(tile.get("y"), f"{twhere}: y", 0, rows - 1)
            w = _int(tile.get("w", 1), f"{twhere}: width", 1, cols - x)
            h = _int(tile.get("h", 1), f"{twhere}: height", 1, rows - y)

            for cy in range(y, y + h):
                for cx in range(x, x + w):
                    if (other := grid[cy][cx]) is not None:
                        raise LayoutError(
                            f'{twhere} overlaps tile "{other}" at column {cx + 1}, '
                            f"row {cy + 1}"
                        )
                    grid[cy][cx] = tile_id

            tiles.append(
                {
                    "id": tile_id,
                    "kind": kind,
                    "entity_id": entity_id,
                    "label": label,
                    "x": x,
                    "y": y,
                    "w": w,
                    "h": h,
                    "icon": tile_icon,
                    **sensors,
                }
            )

        pages.append(
            {
                "id": page_id,
                "title": title,
                "icon": icon,
                "cols": cols,
                "rows": rows,
                "tiles": tiles,
            }
        )

    return {"version": LAYOUT_VERSION, "pages": pages}


def upgrade_layout(layout: dict[str, Any]) -> dict[str, Any]:
    """Fill fields added after a layout was stored (page and tile icons, sensor fields)."""
    for page in layout.get("pages", []):
        page.setdefault("icon", "")
    for tile in iter_tiles(layout):
        tile.setdefault("icon", "")
        for fld in SENSOR_FIELDS:
            tile.setdefault(fld, "")
    return layout


def iter_tiles(layout: dict[str, Any]) -> list[dict[str, Any]]:
    """Return all tiles of a layout."""
    return [tile for page in layout.get("pages", []) for tile in page.get("tiles", [])]
