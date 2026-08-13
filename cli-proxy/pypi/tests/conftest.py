"""Make the build hook importable from the tests directory.

``hatch_build.py`` sits at the package root because hatchling loads it by path
rather than as an installed module, so it is not importable from ``tests/``
without help.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
