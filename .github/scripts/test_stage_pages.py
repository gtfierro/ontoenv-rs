"""Exercise documentation publication with distinct version contents."""

import tempfile
import unittest
from pathlib import Path

from stage_pages import stage


class StagePagesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.sequence = 0
        self.history = self.root / "missing-history"

    def publish(self, text, tag=""):
        self.sequence += 1
        build = self.root / f"build-{self.sequence}"
        build.mkdir()
        (build / "nested").mkdir()
        (build / "index.html").write_text(f"<html><body>{text}</body></html>")
        (build / "nested/page.html").write_text(f"<html><body>nested {text}</body></html>")
        output = self.root / f"site-{self.sequence}"
        manifest = stage(build, self.history, output, tag)
        self.history = output
        return output, manifest

    def test_development_and_rc_preserve_stable_and_each_other(self):
        self.publish("stable API", "v0.6.4")
        self.publish("development API")
        output, manifest = self.publish("RC API", "v0.7.0-rc.1")
        self.assertIn("stable API", (output / "index.html").read_text())
        self.assertIn("development API", (output / "dev/index.html").read_text())
        self.assertIn("RC API", (output / "versions/v0.7.0-rc.1/index.html").read_text())
        self.assertEqual(manifest["default"], "versions/v0.6.4/")
        output, _ = self.publish("new development API")
        self.assertIn("stable API", (output / "index.html").read_text())
        self.assertIn("RC API", (output / "versions/v0.7.0-rc.1/index.html").read_text())

    def test_backfilling_an_older_release_does_not_change_latest_stable(self):
        self.publish("new stable", "v0.10.0")
        output, manifest = self.publish("old stable", "v0.9.0")
        self.assertEqual(manifest["default"], "versions/v0.10.0/")
        self.assertIn("new stable", (output / "index.html").read_text())
        self.assertIn("old stable", (output / "versions/v0.9.0/index.html").read_text())

    def test_nested_pages_and_old_versions_get_one_shared_switcher(self):
        self.publish("stable", "v0.6.4")
        output, _ = self.publish("RC", "v0.7.0-rc.1")
        page = (output / "versions/v0.6.4/nested/page.html").read_text()
        self.assertIn('src="../../../_static/version-switcher.js"', page)
        self.assertEqual(page.count("ontoenv-versions:start"), 1)
        self.assertTrue((output / "_static/version-switcher.js").is_file())
        self.assertIn('href="../versions/v0.6.4/"', (output / "versions/index.html").read_text())

    def test_first_rc_is_labelled_prerelease_without_a_stable_version(self):
        output, manifest = self.publish("first RC", "v0.7.0-rc.1")
        self.assertEqual(manifest["versions"][0]["kind"], "prerelease")
        self.assertEqual(manifest["default"], "versions/v0.7.0-rc.1/")
        self.assertIn("first RC", (output / "index.html").read_text())

    def test_invalid_tag_cannot_replace_existing_history(self):
        output, _ = self.publish("stable", "v0.6.4")
        before = (output / "index.html").read_text()
        for tag in ("../main", "v0.7.0/../../dev", "--delete", "v0.7.0;true"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                self.publish("bad", tag)
        self.assertEqual((output / "index.html").read_text(), before)

    def test_overlapping_paths_are_rejected(self):
        output, _ = self.publish("stable", "v0.6.4")
        with self.assertRaises(ValueError):
            stage(output, self.root / "missing", output / "child")


if __name__ == "__main__":
    unittest.main()
