import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('release_config', Path(__file__).with_name('release_config.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ReleaseConfigTests(unittest.TestCase):
    def test_standalone_compose_and_env_match_saved_image_tag(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp)
            image = module.render('a' * 40, output)
            compose = (output / 'compose.yaml').read_text()
            env = (output / 'container.env.example').read_text()
            self.assertIn('    image: ' + image + '\n', compose)
            self.assertNotIn('    build:', compose)
            self.assertNotIn('TOKENGO_IMAGE', compose)
            self.assertNotIn('TOKENGO_IMAGE=', env)
            self.assertIn('Image is fixed in compose.yaml', env)
            self.assertIn('tokengo-data:/data', compose)
            self.assertIn('read_only: true', compose)
            self.assertIn('TOKENGO_BIND:-127.0.0.1', compose)

    def test_invalid_or_partial_commit_creates_no_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / 'out'
            for commit in ['abc123', 'z' * 40, 'A' * 40]:
                with self.assertRaises(ValueError):
                    module.render(commit, output)
            self.assertFalse(output.exists())

    def test_changed_template_fails_instead_of_shipping_wrong_tag(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'compose.yaml').write_text('services: {}\n')
            with self.assertRaises(ValueError):
                module.render('a' * 40, root / 'out', root)
            self.assertFalse((root / 'out').exists())


if __name__ == '__main__':
    unittest.main()
