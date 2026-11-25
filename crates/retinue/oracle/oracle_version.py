"""Check the stock versions exercised by the current and previous oracle campaigns."""

SUPPORTED_RNS = ("1.5.7", "1.6.0", "1.6.1")


def require_rns(version: str) -> None:
    if version not in SUPPORTED_RNS:
        raise RuntimeError(f"expected stock RNS {' or '.join(SUPPORTED_RNS)}, got {version}")
