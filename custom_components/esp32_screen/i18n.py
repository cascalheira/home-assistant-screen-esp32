"""Tile texts in the screen's language (docs/protocol.md, section 3).

View models are built in English; for a screen in Portuguese (Portugal) the
status and detail texts are translated before they are sent. Values such as
"21.5 °C" or an artist and title pass through unchanged.
"""

from __future__ import annotations

import re
from typing import Any

PT: dict[str, str] = {
    "On": "Ligado",
    "Off": "Desligado",
    "Open": "Aberto",
    "Opening": "A abrir",
    "Closed": "Fechado",
    "Closing": "A fechar",
    "Stopped": "Parado",
    "Unavailable": "Indisponível",
    "Unknown": "Desconhecido",
    "Enabled": "Ativa",
    "Disabled": "Desativada",
    "Run": "Executar",
    "Running": "A executar",
    "Press": "Premir",
    "Never ran": "Nunca executada",
    "Ran just now": "Executada agora",
    "Detected": "Detetado",
    "Clear": "Sem deteção",
    "Wet": "Molhado",
    "Dry": "Seco",
    "Locked": "Trancada",
    "Unlocked": "Destrancada",
    "Locking": "A trancar",
    "Unlocking": "A destrancar",
    "Jammed": "Encravada",
    "Disarmed": "Desarmado",
    "Disarming": "A desarmar",
    "Arming": "A armar",
    "Armed home": "Armado em casa",
    "Armed away": "Armado ausente",
    "Armed night": "Armado noite",
    "Armed vacation": "Armado férias",
    "Armed custom": "Armado personalizado",
    "Pending": "Pendente",
    "Triggered": "Disparado",
    "Playing": "A reproduzir",
    "Paused": "Em pausa",
    "Buffering": "A carregar",
    "Idle": "Inativo",
    "Standby": "Em espera",
    "Cooling": "A arrefecer",
    "Heating": "A aquecer",
    "Drying": "A desumidificar",
    "Fan": "Ventilação",
    "Preheating": "A pré-aquecer",
    "Defrosting": "A descongelar",
    "Heat": "Aquecer",
    "Cool": "Arrefecer",
    "Heat/Cool": "Aquecer/Arrefecer",
    "Auto": "Automático",
    "Fan only": "Só ventilação",
}

_RAN = re.compile(r"^Ran (\d+) (min|h|d) ago$")


def localize(vm: dict[str, Any], language: str) -> dict[str, Any]:
    """`vm` with its status and detail in `language` ("en" leaves it)."""
    if language != "pt":
        return vm
    out = dict(vm)
    for key in ("status", "detail"):
        text = out.get(key)
        if not isinstance(text, str) or not text:
            continue
        if text in PT:
            out[key] = PT[text]
        elif (m := _RAN.match(text)) is not None:
            out[key] = f"Executada há {m.group(1)} {m.group(2)}"
    return out
