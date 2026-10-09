"""Split contiguous extractor samples by whole game, without importing PyTorch."""
import random


def split_game_ranges(meta, val_frac, seed=0):
    """Return (train, validation) lists of half-open sample ranges.

    `val_frac` selects a fraction of games, so sample proportions can differ.
    Old shards must be re-extracted: their game boundaries cannot be recovered.
    """
    shards = meta.get("shard_game_samples")
    if shards is None:
        raise ValueError("Dataset lacks game boundaries; re-extract with the current extractor")
    if not 0 < val_frac < 1:
        raise ValueError("val-frac must be between 0 and 1")
    if len(shards) != len(meta["shard_samples"]):
        raise ValueError("Game boundary shard count mismatch")
    ranges = []
    offset = 0
    for counts, expected in zip(shards, meta["shard_samples"], strict=True):
        if any(type(count) is not int or count <= 0 for count in counts):
            raise ValueError("Game sample counts must be positive integers")
        if sum(counts) != expected:
            raise ValueError("Game sample counts do not match shard size")
        for count in counts:
            ranges.append((offset, offset + count))
            offset += count
    if offset != meta["total_samples"]:
        raise ValueError("Game sample counts do not match total size")
    if len(ranges) < 2:
        raise ValueError("At least two non-empty games are required for validation")
    n_val = max(1, min(len(ranges) - 1, int(len(ranges) * val_frac)))
    val_games = set(random.Random(seed).sample(range(len(ranges)), n_val))
    train = [r for i, r in enumerate(ranges) if i not in val_games]
    val = [r for i, r in enumerate(ranges) if i in val_games]
    return train, val
