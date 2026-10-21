#!/usr/bin/env python3
"""Render standalone release Compose with the same tag as the saved image."""
import argparse
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
SOURCE_IMAGE_AND_BUILD = '''    image: ${TOKENGO_IMAGE:-tokengo:local}
    build:
      context: .
      target: runtime
      args:
        VCS_REF: ${TOKENGO_REVISION:-unknown}
'''


def render(commit, output, root=ROOT):
    if not re.fullmatch(r'[0-9a-f]{40}', commit):
        raise ValueError('Expected full lowercase source commit')
    image = 'ghcr.io/qiwolf/tokengo:sha-' + commit
    compose = (root / 'compose.yaml').read_text()
    if compose.count(SOURCE_IMAGE_AND_BUILD) != 1:
        raise ValueError('Source Compose image/build template changed; review release rendering')
    compose = compose.replace(SOURCE_IMAGE_AND_BUILD, '    image: ' + image + '\n')
    env = (root / 'deploy/container.env.example').read_text()
    if env.count('TOKENGO_IMAGE=tokengo:local\n') != 1:
        raise ValueError('Source image default changed; review release rendering')
    env = env.replace('TOKENGO_IMAGE=tokengo:local\n', '# Image is fixed in compose.yaml; edit its image line to change versions.\n')
    env = env.replace('# Set to the reviewed git commit before building.\nTOKENGO_REVISION=unknown\n', '')
    output.mkdir(parents=True, exist_ok=True)
    (output / 'compose.yaml').write_text(compose)
    (output / 'container.env.example').write_text(env)
    return image


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('commit')
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    print(render(args.commit, args.output))
