import unittest

from score_gallery import score


class GalleryScoringTests(unittest.TestCase):
    def test_off_grid_observation_cannot_create_expected_return(self):
        tile = {"person": 1, "rect": [0, 0, 8, 8], "kind": "video", "role": "primary", "name": "GUEST"}
        region = {
            "rect": {"x": 0, "y": 0, "width": 1, "height": 1},
            "participant_id": 1, "appearance_id": 1,
            "kind": "video", "role": "primary", "identity_uncertain": False,
        }
        truth = {
            "case": "continuous", "width": 8, "height": 8, "fps": 10,
            "frames": [{"frame": index, "tiles": [tile]} for index in (0, 1)],
        }
        analysis = {
            "observations": [
                {"timestamp": {"pts": tick, "time_base": {"numerator": 1, "denominator": 20}},
                 "regions": [region]} for tick in (0, 1, 2)
            ],
            "events": [],
        }

        result = score(truth, analysis)
        self.assertEqual(result["extra_observation_timestamps"], 1)
        self.assertEqual(result["false_boxes"], 1)
        self.assertEqual(result["expected_returns"], 0)
        self.assertEqual(result["unmatched_returns"], 0)


if __name__ == "__main__":
    unittest.main()
