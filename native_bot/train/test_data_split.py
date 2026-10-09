import unittest

from data_split import split_game_ranges


class GameSplitTest(unittest.TestCase):
    def test_games_never_cross_partitions(self):
        meta = {"shard_samples": [5, 0, 10], "shard_game_samples": [[2, 3], [], [1, 4, 5]], "total_samples": 15}
        train, val = split_game_ranges(meta, 0.4)
        train_samples = {i for start, end in train for i in range(start, end)}
        val_samples = {i for start, end in val for i in range(start, end)}
        self.assertFalse(train_samples & val_samples)
        self.assertEqual(train_samples | val_samples, set(range(15)))
        self.assertEqual(len(val), 2)
        self.assertEqual((train, val), split_game_ranges(meta, 0.4))
        for start, end in [(0, 2), (2, 5), (5, 6), (6, 10), (10, 15)]:
            game = set(range(start, end))
            self.assertTrue(game <= train_samples or game <= val_samples)
        self.assertTrue(split_game_ranges(meta, 0.0001)[1])
        self.assertTrue(split_game_ranges(meta, 0.9999)[0])

    def test_old_and_corrupt_metadata_fail_loudly(self):
        cases = [
            {"total_samples": 2, "shard_samples": [2]},
            {"total_samples": 2, "shard_samples": [2], "shard_game_samples": []},
            {"total_samples": 3, "shard_samples": [3], "shard_game_samples": [[1, 1]]},
            {"total_samples": 3, "shard_samples": [2], "shard_game_samples": [[1, 1]]},
            {"total_samples": 2, "shard_samples": [2], "shard_game_samples": [[0, 2]]},
            {"total_samples": 2, "shard_samples": [2], "shard_game_samples": [[2]]},
        ]
        for meta in cases:
            with self.subTest(meta=meta), self.assertRaises(ValueError):
                split_game_ranges(meta, 0.02)

    def test_invalid_validation_fractions(self):
        meta = {"total_samples": 2, "shard_samples": [2], "shard_game_samples": [[1, 1]]}
        for frac in [0, 1, -1, float("nan")]:
            with self.subTest(frac=frac), self.assertRaises(ValueError):
                split_game_ranges(meta, frac)


if __name__ == "__main__":
    unittest.main()
